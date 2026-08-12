//! Encoding text to a QR matrix and rendering it for a terminal.

use qrcode::{Color, EcLevel, QrCode};

/// QR error-correction levels, lowest (most capacity) to highest (most redundancy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorLevel {
    L,
    M,
    Q,
    H,
}

impl From<ErrorLevel> for EcLevel {
    fn from(level: ErrorLevel) -> Self {
        match level {
            ErrorLevel::L => EcLevel::L,
            ErrorLevel::M => EcLevel::M,
            ErrorLevel::Q => EcLevel::Q,
            ErrorLevel::H => EcLevel::H,
        }
    }
}

/// Produce the QR module matrix for `text`. `true` is a dark module.
pub fn encode_matrix(text: &str, level: ErrorLevel) -> Result<Vec<Vec<bool>>, String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), level.into())
        .map_err(|err| err.to_string())?;
    let width = code.width();
    let colors = code.to_colors();
    Ok(colors
        .chunks(width)
        .map(|row| row.iter().map(|c| *c == Color::Dark).collect())
        .collect())
}

/// Render a module matrix to terminal lines using vertical half-blocks so each
/// text row carries two module rows. Light modules are drawn as the visible
/// block (and dark as empty), which keeps the code scannable on the dark
/// terminal backgrounds that are the common default.
///
/// A quiet zone of `margin` light modules is added around the matrix so the
/// finder patterns aren't flush against surrounding text.
pub fn render_qr_lines(matrix: &[Vec<bool>], margin: usize) -> Vec<String> {
    let size = matrix.len();
    let dim = size + margin * 2;

    // Dark inside the data area, light everywhere in the quiet zone.
    let is_dark = |r: usize, c: usize| -> bool {
        if r < margin || c < margin {
            return false;
        }
        let (mr, mc) = (r - margin, c - margin);
        if mr >= size || mc >= size {
            return false;
        }
        matrix[mr][mc]
    };

    (0..dim)
        .step_by(2)
        .map(|r| {
            (0..dim)
                .map(|c| {
                    // Rows past the matrix (when `dim` is odd) read as the
                    // light quiet zone.
                    let top = is_dark(r, c);
                    let bottom = r + 1 < dim && is_dark(r + 1, c);
                    glyph(top, bottom)
                })
                .collect()
        })
        .collect()
}

fn glyph(top_dark: bool, bottom_dark: bool) -> char {
    match (top_dark, bottom_dark) {
        (false, false) => '█', // both light
        (false, true) => '▀',  // light on top only
        (true, false) => '▄',  // light on bottom only
        (true, true) => ' ',   // both dark
    }
}

/// Encode `text` and render it to terminal lines in one step.
pub fn to_qr_lines(text: &str) -> Result<Vec<String>, String> {
    Ok(render_qr_lines(&encode_matrix(text, ErrorLevel::M)?, 1))
}

/// Decide what to encode: an explicit positional argument wins; otherwise fall
/// back to piped stdin (with its trailing whitespace trimmed). `None` when
/// neither yields any content.
pub fn resolve_qr_input(args: &[String], stdin: Option<&str>) -> Option<String> {
    let from_args = args.join(" ").trim().to_string();
    if !from_args.is_empty() {
        return Some(from_args);
    }
    let from_stdin = stdin?.trim_end().to_string();
    (!from_stdin.is_empty()).then_some(from_stdin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn maps_each_module_pair_to_the_right_half_block() {
        // One text row from two module rows. Light modules render as blocks.
        let matrix = vec![
            vec![true, false], // dark, light
            vec![false, true], // light, dark
        ];
        // col0: top dark + bottom light → '▄'; col1: top light + bottom dark → '▀'.
        assert_eq!(render_qr_lines(&matrix, 0), vec!["▄▀"]);
    }

    #[test]
    fn renders_an_all_dark_matrix_as_blank_cells() {
        let matrix = vec![vec![true, true], vec![true, true]];
        assert_eq!(render_qr_lines(&matrix, 0), vec!["  "]);
    }

    #[test]
    fn surrounds_the_matrix_with_a_light_quiet_zone() {
        let lines = render_qr_lines(&[vec![true]], 1);
        // 1x1 dark + margin 1 → 3x3 grid → 2 text rows; the dark cell is at (1,1).
        assert_eq!(lines.len(), 2);
        // Row 0 (light) over row 1 (light, dark, light) → centre is light-on-top.
        assert_eq!(lines[0], "█▀█");
        // Row 2 (light) over the phantom light row → all blocks.
        assert_eq!(lines[1], "███");
    }

    #[test]
    fn treats_the_phantom_row_past_an_odd_grid_as_light() {
        // size 1 + margin 0 → dim 1 (odd): single module, no row below it.
        assert_eq!(render_qr_lines(&[vec![true]], 0), vec!["▄"]);
    }

    #[test]
    fn produces_a_square_matrix_with_a_dark_finder_pattern() {
        let matrix = encode_matrix("hello", ErrorLevel::M).unwrap();
        assert!(!matrix.is_empty());
        assert_eq!(matrix.len(), matrix[0].len()); // square
                                                   // The top-left finder pattern starts with a dark module.
        assert!(matrix[0][0]);
    }

    #[test]
    fn every_error_level_encodes() {
        for level in [ErrorLevel::L, ErrorLevel::M, ErrorLevel::Q, ErrorLevel::H] {
            let matrix = encode_matrix("https://example.com", level).unwrap();
            assert_eq!(matrix.len(), matrix[0].len());
        }
    }

    #[test]
    fn higher_error_correction_needs_at_least_as_many_modules() {
        let low = encode_matrix("hello world", ErrorLevel::L).unwrap().len();
        let high = encode_matrix("hello world", ErrorLevel::H).unwrap().len();
        assert!(high >= low);
    }

    #[test]
    fn refuses_input_too_large_to_encode() {
        let huge = "x".repeat(10_000);
        assert!(encode_matrix(&huge, ErrorLevel::M).is_err());
    }

    #[test]
    fn renders_a_full_code_with_a_quiet_zone() {
        let lines = to_qr_lines("https://example.com").unwrap();
        let matrix = encode_matrix("https://example.com", ErrorLevel::M).unwrap();
        let dim = matrix.len() + 2; // margin of 1 on each side
        assert_eq!(lines.len(), dim.div_ceil(2));
        assert!(lines.iter().all(|l| l.chars().count() == dim));
    }

    #[test]
    fn prefers_positional_args_joined_with_a_space() {
        assert_eq!(
            resolve_qr_input(&args(&["hello", "world"]), Some("piped")).as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn falls_back_to_stdin_and_trims_trailing_whitespace() {
        assert_eq!(
            resolve_qr_input(&[], Some("https://example.com\n")).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn ignores_whitespace_only_args_before_using_stdin() {
        assert_eq!(
            resolve_qr_input(&args(&["   "]), Some("frompipe")).as_deref(),
            Some("frompipe")
        );
    }

    #[test]
    fn returns_nothing_when_neither_source_has_content() {
        assert!(resolve_qr_input(&[], Some("\n")).is_none());
        assert!(resolve_qr_input(&[], None).is_none());
    }
}
