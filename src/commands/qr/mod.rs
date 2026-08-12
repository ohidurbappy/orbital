//! `orbital qr` — encode an argument, piped stdin, or an interactively built
//! payload into a QR code drawn in the terminal.

pub mod encode;
pub mod types;

use encode::{resolve_qr_input, to_qr_lines};
use types::{QrType, Values, QR_TYPES};

use crate::commands::{Command, Ctx, Stdin};
use crate::core::version::BIN;
use crate::style;
use crate::term::{self, Frame, Key, RawMode};
use crate::Res;

pub const COMMAND: Command = Command {
    name: "qr",
    description: "Encode text (argument or piped stdin) into a QR code",
    aliases: &["qrcode"],
    run: None,
    view,
    stdin: Stdin::WhenNoArgs,
};

/// Width of the label column while filling in fields.
const LABEL_WIDTH: usize = 26;

fn view(ctx: &Ctx) -> Res {
    // A payload from args or a pipe skips the builder entirely.
    if let Some(text) = resolve_qr_input(ctx.args, ctx.input) {
        term::emit(&result_lines(&text));
        return Ok(());
    }
    if !ctx.interactive {
        term::emit(&usage_lines());
        return Ok(());
    }
    build_interactively()
}

fn build_interactively() -> Res {
    let _raw = RawMode::enable()?;
    let mut frame = Frame::new();
    let mut state = Builder::default();

    let payload = loop {
        frame.draw(&state.render())?;
        let key = term::read_key()?;

        match state.stage {
            Stage::Pick => match key {
                Key::Escape | Key::Interrupt => {
                    frame.clear()?;
                    return Ok(());
                }
                Key::Up => state.type_index = state.type_index.saturating_sub(1),
                Key::Down => state.type_index = (state.type_index + 1).min(QR_TYPES.len() - 1),
                Key::Enter => state.start_filling(),
                _ => {}
            },
            Stage::Fill => match key {
                Key::Interrupt => {
                    frame.clear()?;
                    return Ok(());
                }
                Key::Escape => state.back_to_pick(),
                Key::Enter => {
                    if let Some(payload) = state.commit_field() {
                        break payload;
                    }
                }
                Key::Backspace => {
                    state.buffer.pop();
                }
                Key::Char(c) => state.buffer.push(c),
                _ => {}
            },
        }
    };

    // Leave the finished code on screen, with a dismissable prompt below it.
    frame.draw(&result_lines(&payload))?;
    frame.keep();

    let mut hint = Frame::new();
    hint.draw(&[style::dim("Press any key to exit.")])?;
    let _ = term::read_key()?;
    hint.clear()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Pick,
    Fill,
}

struct Builder {
    stage: Stage,
    type_index: usize,
    field_index: usize,
    values: Values,
    buffer: String,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            stage: Stage::Pick,
            type_index: 0,
            field_index: 0,
            values: Values::new(),
            buffer: String::new(),
        }
    }
}

impl Builder {
    fn qr_type(&self) -> &'static QrType {
        &QR_TYPES[self.type_index]
    }

    fn start_filling(&mut self) {
        self.stage = Stage::Fill;
        self.values.clear();
        self.buffer.clear();
        self.field_index = 0;
    }

    fn back_to_pick(&mut self) {
        self.stage = Stage::Pick;
        self.values.clear();
        self.buffer.clear();
        self.field_index = 0;
    }

    /// Store the current field. Returns the built payload once the last field
    /// is filled, or `None` while more remain (or the entry was rejected).
    fn commit_field(&mut self) -> Option<String> {
        let qr_type = self.qr_type();
        let field = &qr_type.fields[self.field_index];
        // A required field can't be empty.
        if self.buffer.trim().is_empty() && !field.optional {
            return None;
        }

        self.values
            .insert(field.key.to_string(), std::mem::take(&mut self.buffer));

        if self.field_index + 1 < qr_type.fields.len() {
            self.field_index += 1;
            None
        } else {
            Some((qr_type.build)(&self.values))
        }
    }

    fn render(&self) -> Vec<String> {
        match self.stage {
            Stage::Pick => self.render_pick(),
            Stage::Fill => self.render_fill(),
        }
    }

    fn render_pick(&self) -> Vec<String> {
        let mut lines = vec![
            style::bold_cyan("What kind of QR code?"),
            style::dim("↑/↓ to move · Enter to select · Esc to quit"),
            String::new(),
        ];
        for (i, qr_type) in QR_TYPES.iter().enumerate() {
            let selected = i == self.type_index;
            let marker = if selected {
                style::green("❯ ")
            } else {
                "  ".to_string()
            };
            let label = style::pad(qr_type.label, 14);
            let label = if selected {
                style::bold_green(&label)
            } else {
                style::cyan(&label)
            };
            let hint = if selected {
                qr_type.hint.to_string()
            } else {
                style::dim(qr_type.hint)
            };
            lines.push(format!("{marker}{label}{hint}"));
        }
        lines
    }

    fn render_fill(&self) -> Vec<String> {
        let qr_type = self.qr_type();
        let field = &qr_type.fields[self.field_index];

        let mut lines = vec![
            format!(
                "{}{}",
                style::bold_cyan(qr_type.label),
                style::dim(" — Enter to confirm · Esc to go back")
            ),
            String::new(),
        ];

        for done in &qr_type.fields[..self.field_index] {
            let value = self.values.get(done.key).cloned().unwrap_or_default();
            let value = if value.is_empty() {
                style::dim("(skipped)")
            } else {
                value
            };
            lines.push(format!(
                "{}{value}",
                style::dim(&style::pad(done.label, LABEL_WIDTH))
            ));
        }

        let label = format!(
            "{}{}",
            field.label,
            if field.optional { " (optional)" } else { "" }
        );
        let mut current = format!(
            "{}{}{}",
            style::bold_green(&style::pad(&label, LABEL_WIDTH)),
            self.buffer,
            style::green("▏")
        );
        if self.buffer.is_empty() {
            if let Some(placeholder) = field.placeholder {
                current.push_str(&style::dim(&format!(" e.g. {placeholder}")));
            }
        }
        lines.push(current);
        lines
    }
}

/// The finished code, with the encoded payload printed underneath it.
fn result_lines(text: &str) -> Vec<String> {
    let mut lines = match to_qr_lines(text) {
        Ok(lines) => lines,
        Err(message) => return vec![style::red(&format!("Could not encode QR: {message}"))],
    };
    lines.push(String::new());
    lines.push(style::dim(text));
    lines
}

fn usage_lines() -> Vec<String> {
    vec![
        style::yellow("Nothing to encode."),
        style::dim(&format!("Usage: {BIN} qr <text>")),
        style::dim(&format!("   or: echo \"text\" | {BIN} qr")),
        style::dim("Run in a terminal with no argument to pick a type interactively."),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder() -> Builder {
        Builder::default()
    }

    fn type_index_of(label: &str) -> usize {
        QR_TYPES.iter().position(|t| t.label == label).unwrap()
    }

    #[test]
    fn the_picker_lists_every_type() {
        let lines = builder().render_pick().join("\n");
        for qr_type in QR_TYPES {
            assert!(lines.contains(qr_type.label), "{} missing", qr_type.label);
            assert!(lines.contains(qr_type.hint));
        }
    }

    #[test]
    fn the_picker_marks_the_highlighted_type() {
        let mut state = builder();
        state.type_index = 1;
        let lines = state.render_pick();
        // Two header lines plus a blank precede the list.
        assert!(lines[3 + 1].starts_with("❯ "));
        assert!(lines[3].starts_with("  "));
    }

    #[test]
    fn a_required_field_rejects_an_empty_entry() {
        let mut state = builder();
        state.start_filling();
        assert!(state.commit_field().is_none());
        assert_eq!(state.field_index, 0, "should stay on the same field");
    }

    #[test]
    fn an_optional_field_accepts_an_empty_entry() {
        let mut state = builder();
        state.type_index = type_index_of("SMS");
        state.start_filling();
        state.buffer.push_str("+1555");
        assert!(state.commit_field().is_none()); // moves to the message field
        assert_eq!(state.field_index, 1);
        // Blank message is allowed and completes the payload.
        assert_eq!(state.commit_field().as_deref(), Some("SMSTO:+1555"));
    }

    #[test]
    fn filling_the_last_field_builds_the_payload() {
        let mut state = builder();
        state.type_index = type_index_of("URL");
        state.start_filling();
        state.buffer.push_str("example.com");
        assert_eq!(state.commit_field().as_deref(), Some("https://example.com"));
    }

    #[test]
    fn going_back_clears_everything_entered() {
        let mut state = builder();
        state.start_filling();
        state.buffer.push_str("half typed");
        state.values.insert("text".into(), "old".into());
        state.back_to_pick();
        assert_eq!(state.stage, Stage::Pick);
        assert!(state.buffer.is_empty());
        assert!(state.values.is_empty());
        assert_eq!(state.field_index, 0);
    }

    #[test]
    fn the_fill_view_shows_completed_fields_and_the_current_prompt() {
        let mut state = builder();
        state.type_index = type_index_of("Email");
        state.start_filling();
        state.buffer.push_str("a@b.com");
        state.commit_field();
        let lines = state.render_fill().join("\n");
        assert!(
            lines.contains("a@b.com"),
            "completed field should stay visible"
        );
        assert!(lines.contains("Subject (optional)"));
    }

    #[test]
    fn skipped_optional_fields_are_marked() {
        let mut state = builder();
        state.type_index = type_index_of("Email");
        state.start_filling();
        state.buffer.push_str("a@b.com");
        state.commit_field();
        state.commit_field(); // skip the subject
        assert!(state.render_fill().join("\n").contains("(skipped)"));
    }

    #[test]
    fn the_placeholder_shows_only_while_the_buffer_is_empty() {
        let mut state = builder();
        state.type_index = type_index_of("URL");
        state.start_filling();
        assert!(state.render_fill().join("\n").contains("e.g. example.com"));
        state.buffer.push('x');
        assert!(!state.render_fill().join("\n").contains("e.g. example.com"));
    }

    #[test]
    fn the_result_prints_the_code_above_its_payload() {
        let lines = result_lines("https://example.com");
        assert!(lines.len() > 2);
        assert_eq!(lines.last().unwrap(), "https://example.com");
    }

    #[test]
    fn an_unencodable_payload_reports_the_failure() {
        let lines = result_lines(&"x".repeat(10_000));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Could not encode QR"));
    }

    #[test]
    fn usage_names_both_input_routes() {
        let usage = usage_lines().join("\n");
        assert!(usage.contains("orbital qr <text>"));
        assert!(usage.contains("| orbital qr"));
    }
}
