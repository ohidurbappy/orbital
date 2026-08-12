//! A single aligned `label  value` row, reused across command output.

use crate::style;

/// Width reserved for the label column so values line up.
pub const LABEL_WIDTH: usize = 12;

pub fn key_value(label: &str, value: &str) -> String {
    key_value_with(label, value, LABEL_WIDTH, "cyan")
}

pub fn key_value_with(label: &str, value: &str, label_width: usize, color: &str) -> String {
    let padded = style::pad(label, label_width);
    format!("{}{value}", style::bold(&style::colored(&padded, color)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_values_at_the_label_width() {
        // Colour is off under test, so the row is the plain padded text.
        assert_eq!(key_value("OS", "Ubuntu"), "OS          Ubuntu");
    }

    #[test]
    fn long_labels_still_render_their_value() {
        let row = key_value("a-very-long-label", "v");
        assert!(row.ends_with("v"));
    }
}
