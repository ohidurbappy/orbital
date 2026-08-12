//! Terminal plumbing for the interactive views: raw mode, key events, and
//! in-place frame redrawing.
//!
//! This is the Rust stand-in for what Ink did in the TypeScript version — a
//! view builds a `Vec<String>` of lines and hands it to [`Frame::draw`], which
//! repaints over the previous frame instead of scrolling the terminal.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, queue, terminal};

/// True when a human is watching stdout, rather than it being a pipe or file.
/// Chrome that isn't part of a command's output — the update banner — is only
/// worth drawing when this holds.
pub fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

/// True when we can both draw a UI and read keystrokes.
pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && stdout_is_tty()
}

/// Enables raw mode for its lifetime and restores the terminal on drop —
/// including on panic, so a crash can't leave the shell without an echo.
pub struct RawMode {
    active: bool,
}

impl RawMode {
    /// Idempotent: when raw mode is already on (a command launched from the
    /// menu, which holds its own guard), this hands back an inert guard so the
    /// inner scope can't drop the outer one out of raw mode.
    pub fn enable() -> io::Result<Self> {
        if terminal::is_raw_mode_enabled()? {
            return Ok(Self { active: false });
        }
        terminal::enable_raw_mode()?;
        Ok(Self { active: true })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if self.active {
            let _ = terminal::disable_raw_mode();
        }
    }
}

/// The keys the interactive views care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
    /// Ctrl-C, or any other request to quit outright.
    Interrupt,
    /// A printable character typed by the user.
    Char(char),
    /// Something we don't handle (function keys, resize, mouse, …).
    Other,
}

fn classify(key: KeyEvent) -> Key {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace | KeyCode::Delete => Key::Backspace,
        KeyCode::Char('c') if ctrl => Key::Interrupt,
        KeyCode::Char('d') if ctrl => Key::Interrupt,
        // Emacs-style motion, matching the TypeScript menu's Ctrl-P/Ctrl-N.
        KeyCode::Char('p') if ctrl => Key::Up,
        KeyCode::Char('n') if ctrl => Key::Down,
        KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => Key::Char(c),
        _ => Key::Other,
    }
}

/// Block until the user presses a key we recognise.
pub fn read_key() -> io::Result<Key> {
    loop {
        if let Event::Key(key) = event::read()? {
            // Windows reports press *and* release; only act on the press.
            if key.kind != KeyEventKind::Press {
                continue;
            }
            return Ok(classify(key));
        }
    }
}

/// Like [`read_key`], but gives up after `timeout` so a caller can refresh
/// itself on a timer. Returns `None` when nothing was pressed.
pub fn read_key_timeout(timeout: Duration) -> io::Result<Option<Key>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(classify(key))),
        _ => Ok(None),
    }
}

/// Raw mode swallows the implicit carriage return, so lines need `\r\n` there.
fn newline() -> &'static str {
    if terminal::is_raw_mode_enabled().unwrap_or(false) {
        "\r\n"
    } else {
        "\n"
    }
}

/// A block of lines that can be repainted in place.
#[derive(Default)]
pub struct Frame {
    height: u16,
}

impl Frame {
    pub fn new() -> Self {
        Self { height: 0 }
    }

    /// Repaint the frame with `lines`, erasing whatever was drawn before.
    pub fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        let mut out = io::stdout().lock();
        self.rewind(&mut out)?;
        let newline = newline();
        for line in lines {
            write!(out, "{line}{newline}")?;
        }
        out.flush()?;
        self.height = lines.len() as u16;
        Ok(())
    }

    /// Erase the frame entirely, leaving the cursor where it started.
    pub fn clear(&mut self) -> io::Result<()> {
        let mut out = io::stdout().lock();
        self.rewind(&mut out)?;
        out.flush()?;
        self.height = 0;
        Ok(())
    }

    /// Leave the current frame on screen and stop tracking it, so the next
    /// draw appends below instead of overwriting.
    pub fn keep(&mut self) {
        self.height = 0;
    }

    fn rewind(&self, out: &mut impl Write) -> io::Result<()> {
        if self.height > 0 {
            queue!(
                out,
                cursor::MoveToPreviousLine(self.height),
                terminal::Clear(terminal::ClearType::FromCursorDown)
            )?;
        }
        Ok(())
    }
}

/// Print lines once, with no redraw bookkeeping. Used by every print-and-exit
/// view, and correct whether or not the menu has put us in raw mode.
pub fn emit(lines: &[String]) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let newline = newline();
    for line in lines {
        let _ = write!(out, "{line}{newline}");
    }
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Key {
        classify(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn classifies_navigation_keys() {
        assert_eq!(press(KeyCode::Up, KeyModifiers::NONE), Key::Up);
        assert_eq!(press(KeyCode::Down, KeyModifiers::NONE), Key::Down);
        assert_eq!(press(KeyCode::Enter, KeyModifiers::NONE), Key::Enter);
        assert_eq!(press(KeyCode::Esc, KeyModifiers::NONE), Key::Escape);
    }

    #[test]
    fn maps_ctrl_p_and_n_to_up_and_down() {
        assert_eq!(press(KeyCode::Char('p'), KeyModifiers::CONTROL), Key::Up);
        assert_eq!(press(KeyCode::Char('n'), KeyModifiers::CONTROL), Key::Down);
    }

    #[test]
    fn ctrl_c_interrupts() {
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Key::Interrupt
        );
    }

    #[test]
    fn plain_characters_are_typed_text() {
        assert_eq!(
            press(KeyCode::Char('q'), KeyModifiers::NONE),
            Key::Char('q')
        );
        // Shifted characters still type — only Ctrl/Alt are treated as commands.
        assert_eq!(
            press(KeyCode::Char('Q'), KeyModifiers::SHIFT),
            Key::Char('Q')
        );
    }

    #[test]
    fn both_delete_keys_erase() {
        assert_eq!(
            press(KeyCode::Backspace, KeyModifiers::NONE),
            Key::Backspace
        );
        assert_eq!(press(KeyCode::Delete, KeyModifiers::NONE), Key::Backspace);
    }
}
