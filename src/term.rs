//! Terminal plumbing for the interactive views: raw mode, key events, and
//! in-place frame redrawing.
//!
//! This is the Rust stand-in for what Ink did in the TypeScript version — a
//! view builds a `Vec<String>` of lines and hands it to [`Frame::draw`], which
//! repaints over the previous frame instead of scrolling the terminal.

use std::io::{self, IsTerminal, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, queue, terminal};

/// True when we can both draw a UI and read keystrokes.
pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
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

/// Raw mode swallows the implicit carriage return, so lines need `\r\n` there.
fn newline() -> &'static str {
    if terminal::is_raw_mode_enabled().unwrap_or(false) {
        "\r\n"
    } else {
        "\n"
    }
}

/// Move the cursor to the start of the line `n` rows up.
fn up(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!("\x1b[{n}F")
    }
}

/// Erase from the cursor to the end of the line.
const CLEAR_TO_EOL: &str = "\x1b[K";

/// Build the escape sequence that turns the `previous` frame into `next`.
///
/// Two properties keep the screen still, and both are what a naive
/// "clear the block, print it again" loop gets wrong:
///
/// * an unchanged frame writes **nothing at all**, so a redraw that changes no
///   pixels costs no flicker;
/// * each row is overwritten in place and trimmed with an erase-to-end-of-line
///   rather than the block being blanked first, so no row is ever empty between
///   the clear and the reprint.
fn repaint(previous: &[String], next: &[String], newline: &str) -> String {
    if previous == next {
        return String::new();
    }

    let mut out = up(previous.len());
    for line in next {
        out.push_str(line);
        out.push_str(CLEAR_TO_EOL);
        out.push_str(newline);
    }

    // A frame that shrank leaves rows below it; wipe them, then step back up so
    // the cursor still sits directly under the frame.
    let extra = previous.len().saturating_sub(next.len());
    for _ in 0..extra {
        out.push_str(CLEAR_TO_EOL);
        out.push_str(newline);
    }
    out.push_str(&up(extra));

    out
}

/// A block of lines that can be repainted in place.
#[derive(Default)]
pub struct Frame {
    drawn: Vec<String>,
}

impl Frame {
    pub fn new() -> Self {
        Self { drawn: Vec::new() }
    }

    /// Repaint the frame with `lines`. A no-op when nothing changed.
    pub fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        let update = repaint(&self.drawn, lines, newline());
        if update.is_empty() {
            return Ok(());
        }
        let mut out = io::stdout().lock();
        out.write_all(update.as_bytes())?;
        out.flush()?;
        self.drawn = lines.to_vec();
        Ok(())
    }

    /// Erase the frame entirely, leaving the cursor where it started.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.drawn.is_empty() {
            return Ok(());
        }
        let mut out = io::stdout().lock();
        queue!(
            out,
            cursor::MoveToPreviousLine(self.drawn.len() as u16),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        out.flush()?;
        self.drawn.clear();
        Ok(())
    }

    /// Leave the current frame on screen and stop tracking it, so the next
    /// draw appends below instead of overwriting.
    pub fn keep(&mut self) {
        self.drawn.clear();
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

    fn lines(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_unchanged_frame_writes_nothing() {
        // The whole point: a redraw that would look identical must not touch
        // the terminal, or the screen flickers at the redraw rate.
        let frame = lines(&["a", "b"]);
        assert_eq!(repaint(&frame, &frame, "\n"), "");
    }

    #[test]
    fn the_first_draw_just_prints_the_lines() {
        assert_eq!(
            repaint(&[], &lines(&["a", "b"]), "\n"),
            "a\x1b[K\nb\x1b[K\n"
        );
    }

    #[test]
    fn a_redraw_steps_back_over_the_previous_frame() {
        let update = repaint(&lines(&["a", "b"]), &lines(&["c", "d"]), "\n");
        assert_eq!(update, "\x1b[2Fc\x1b[K\nd\x1b[K\n");
    }

    #[test]
    fn never_blanks_the_block_before_reprinting_it() {
        // A full-screen erase is what makes a repaint visibly flash.
        let update = repaint(&lines(&["a"]), &lines(&["b"]), "\n");
        assert!(
            !update.contains("\x1b[J"),
            "erases below the cursor: {update:?}"
        );
    }

    #[test]
    fn a_shorter_frame_wipes_the_rows_it_gave_up() {
        let update = repaint(&lines(&["a", "b", "c"]), &lines(&["x"]), "\n");
        // One row printed, two wiped, then back up over the two wiped rows so
        // the cursor still sits under the frame.
        assert_eq!(update, "\x1b[3Fx\x1b[K\n\x1b[K\n\x1b[K\n\x1b[2F");
    }

    #[test]
    fn a_taller_frame_needs_no_step_back() {
        let update = repaint(&lines(&["a"]), &lines(&["x", "y"]), "\n");
        assert_eq!(update, "\x1b[1Fx\x1b[K\ny\x1b[K\n");
    }

    #[test]
    fn raw_mode_lines_carry_their_own_carriage_return() {
        assert_eq!(repaint(&[], &lines(&["a"]), "\r\n"), "a\x1b[K\r\n");
    }

    #[test]
    fn every_row_is_trimmed_so_longer_text_cannot_linger() {
        let update = repaint(&lines(&["a long previous line"]), &lines(&["short"]), "\n");
        assert!(update.contains("short\x1b[K"));
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
