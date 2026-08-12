//! Turning delimited text into an aligned table.
//!
//! Everything here is pure: text in, rendered lines out, so the layout rules
//! are asserted on directly in tests.

use crate::style;

/// How a line is split into cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    /// Split on runs of whitespace, collapsing repeats — the default, and what
    /// makes `ps`/`ls -l`-style output line up.
    Whitespace,
    /// Split on a single character, keeping empty cells (so a CSV's blank
    /// fields survive) and honouring `"quoted, fields"`.
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableFlags {
    pub delimiter: Delimiter,
    /// Treat the first row as column headings.
    pub header: bool,
    /// Draw the frame with `+-|` instead of box-drawing characters.
    pub ascii: bool,
}

impl Default for TableFlags {
    fn default() -> Self {
        Self {
            delimiter: Delimiter::Whitespace,
            header: false,
            ascii: false,
        }
    }
}

/// Parse the option flags `orbital table` accepts from the forwarded CLI
/// tokens. Unrecognised tokens are ignored rather than rejected, matching how
/// the other commands read their flags.
pub fn parse_table_flags(args: &[String]) -> TableFlags {
    let mut flags = TableFlags::default();
    let mut expecting_delimiter = false;

    for arg in args {
        if expecting_delimiter {
            expecting_delimiter = false;
            if let Some(ch) = unescape_delimiter(arg) {
                flags.delimiter = Delimiter::Char(ch);
                continue;
            }
        }

        match arg.as_str() {
            "--csv" => flags.delimiter = Delimiter::Char(','),
            "--tsv" => flags.delimiter = Delimiter::Char('\t'),
            "--header" | "-H" => flags.header = true,
            "--ascii" => flags.ascii = true,
            "-d" | "--delimiter" => expecting_delimiter = true,
            other => {
                // `-d,` / `-d=,` / `--delimiter=,`
                let inline = other
                    .strip_prefix("--delimiter=")
                    .or_else(|| other.strip_prefix("-d="))
                    .or_else(|| other.strip_prefix("-d"));
                if let Some(ch) = inline.and_then(unescape_delimiter) {
                    flags.delimiter = Delimiter::Char(ch);
                }
            }
        }
    }

    flags
}

/// Read a delimiter argument, accepting the usual escapes so `-d '\t'` works
/// from shells that won't pass a literal tab.
fn unescape_delimiter(arg: &str) -> Option<char> {
    match arg {
        "\\t" | "tab" => Some('\t'),
        "\\0" => Some('\0'),
        other => other.chars().next(),
    }
}

/// Split input into rows of cells, skipping blank lines.
pub fn parse_rows(input: &str, delimiter: Delimiter) -> Vec<Vec<String>> {
    input
        .lines()
        // Tolerate CRLF input, which is the norm for Windows pipes and files.
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.trim().is_empty())
        .map(|line| split_line(line, delimiter))
        .collect()
}

fn split_line(line: &str, delimiter: Delimiter) -> Vec<String> {
    match delimiter {
        Delimiter::Whitespace => line.split_whitespace().map(str::to_string).collect(),
        Delimiter::Char(delim) => split_delimited(line, delim),
    }
}

/// Split on `delim`, treating `"…"` as one cell and `""` as an escaped quote.
fn split_delimited(line: &str, delim: char) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            }
            // A quote only opens a field at its start; mid-cell it is literal.
            '"' if cell.is_empty() => quoted = true,
            c if c == delim && !quoted => cells.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    cells.push(cell);
    cells
}

/// Render `rows` as a bordered table. Ragged rows are padded with empty cells
/// so every row spans the full width.
pub fn render_table(rows: &[Vec<String>], flags: TableFlags) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return Vec::new();
    }

    let widths: Vec<usize> = (0..columns)
        .map(|i| {
            rows.iter()
                .map(|row| width(cell(row, i)))
                .max()
                .unwrap_or(0)
        })
        .collect();
    // A column of numbers reads better flush right, the way a spreadsheet
    // would show it. Headings follow their column.
    let body = if flags.header { &rows[1..] } else { rows };
    let right: Vec<bool> = (0..columns)
        .map(|i| is_numeric_column(body.iter().map(|row| cell(row, i))))
        .collect();

    let border = Border::for_flags(flags);
    let mut lines = vec![border.rule(&widths, border.top)];

    for (index, row) in rows.iter().enumerate() {
        let heading = flags.header && index == 0;
        let cells: Vec<String> = (0..columns)
            .map(|i| {
                let text = pad_cell(cell(row, i), widths[i], right[i] && !heading);
                if heading {
                    style::bold_cyan(&text)
                } else {
                    text
                }
            })
            .collect();
        lines.push(format!(
            "{v} {} {v}",
            cells.join(&format!(" {} ", border.vertical)),
            v = border.vertical
        ));
        if heading {
            lines.push(border.rule(&widths, border.middle));
        }
    }

    lines.push(border.rule(&widths, border.bottom));
    lines
}

/// A row's cell, or an empty one when the row is short.
fn cell(row: &[String], index: usize) -> &str {
    row.get(index).map(String::as_str).unwrap_or("")
}

fn pad_cell(text: &str, width_to: usize, right_align: bool) -> String {
    let padding = " ".repeat(width_to.saturating_sub(width(text)));
    if right_align {
        format!("{padding}{text}")
    } else {
        format!("{text}{padding}")
    }
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// True when every non-empty cell in the column parses as a number.
fn is_numeric_column<'a>(cells: impl Iterator<Item = &'a str>) -> bool {
    let mut saw_number = false;
    for cell in cells {
        let cell = cell.trim();
        if cell.is_empty() {
            continue;
        }
        // Reject the float syntax that isn't really tabular data.
        if cell.parse::<f64>().is_err() || cell.contains(['e', 'E', 'i', 'n', 'N']) {
            return false;
        }
        saw_number = true;
    }
    saw_number
}

/// The characters a table frame is drawn with.
struct Border {
    vertical: char,
    horizontal: char,
    top: [char; 3],
    middle: [char; 3],
    bottom: [char; 3],
}

impl Border {
    fn for_flags(flags: TableFlags) -> Self {
        if flags.ascii {
            Self {
                vertical: '|',
                horizontal: '-',
                top: ['+', '+', '+'],
                middle: ['+', '+', '+'],
                bottom: ['+', '+', '+'],
            }
        } else {
            Self {
                vertical: '│',
                horizontal: '─',
                top: ['┌', '┬', '┐'],
                middle: ['├', '┼', '┤'],
                bottom: ['└', '┴', '┘'],
            }
        }
    }

    /// A horizontal rule: `┌───┬───┐` and friends.
    fn rule(&self, widths: &[usize], [left, join, right]: [char; 3]) -> String {
        let segments: Vec<String> = widths
            .iter()
            .map(|w| self.horizontal.to_string().repeat(w + 2))
            .collect();
        format!("{left}{}{right}", segments.join(&join.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn rows(table: &[&[&str]]) -> Vec<Vec<String>> {
        table
            .iter()
            .map(|row| row.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn defaults_to_whitespace_columns() {
        let flags = parse_table_flags(&[]);
        assert_eq!(flags.delimiter, Delimiter::Whitespace);
        assert!(!flags.header);
        assert!(!flags.ascii);
    }

    #[test]
    fn recognizes_the_delimiter_shorthands() {
        assert_eq!(
            parse_table_flags(&args(&["--csv"])).delimiter,
            Delimiter::Char(',')
        );
        assert_eq!(
            parse_table_flags(&args(&["--tsv"])).delimiter,
            Delimiter::Char('\t')
        );
    }

    #[test]
    fn accepts_a_delimiter_as_a_separate_or_attached_value() {
        for form in [
            args(&["-d", ";"]),
            args(&["-d;"]),
            args(&["-d=;"]),
            args(&["--delimiter", ";"]),
            args(&["--delimiter=;"]),
        ] {
            assert_eq!(
                parse_table_flags(&form).delimiter,
                Delimiter::Char(';'),
                "{form:?}"
            );
        }
    }

    #[test]
    fn accepts_an_escaped_tab_delimiter() {
        assert_eq!(
            parse_table_flags(&args(&["-d", "\\t"])).delimiter,
            Delimiter::Char('\t')
        );
    }

    #[test]
    fn recognizes_the_header_and_ascii_flags() {
        let flags = parse_table_flags(&args(&["--header", "--ascii"]));
        assert!(flags.header);
        assert!(flags.ascii);
        assert!(parse_table_flags(&args(&["-H"])).header);
    }

    #[test]
    fn splits_on_runs_of_whitespace() {
        let parsed = parse_rows("a   b\tc\nd e", Delimiter::Whitespace);
        assert_eq!(parsed, rows(&[&["a", "b", "c"], &["d", "e"]]));
    }

    #[test]
    fn skips_blank_lines() {
        let parsed = parse_rows("a\n\n   \nb", Delimiter::Whitespace);
        assert_eq!(parsed, rows(&[&["a"], &["b"]]));
    }

    #[test]
    fn tolerates_crlf_line_endings() {
        let parsed = parse_rows("a,b\r\nc,d\r\n", Delimiter::Char(','));
        assert_eq!(parsed, rows(&[&["a", "b"], &["c", "d"]]));
    }

    #[test]
    fn keeps_empty_cells_when_splitting_on_a_character() {
        let parsed = parse_rows("a,,c", Delimiter::Char(','));
        assert_eq!(parsed, rows(&[&["a", "", "c"]]));
    }

    #[test]
    fn honours_quoted_fields() {
        let parsed = parse_rows("\"last, first\",42", Delimiter::Char(','));
        assert_eq!(parsed, rows(&[&["last, first", "42"]]));
    }

    #[test]
    fn unescapes_doubled_quotes_inside_a_quoted_field() {
        let parsed = parse_rows("\"say \"\"hi\"\"\",x", Delimiter::Char(','));
        assert_eq!(parsed, rows(&[&["say \"hi\"", "x"]]));
    }

    #[test]
    fn a_quote_inside_an_unquoted_field_is_literal() {
        let parsed = parse_rows("12\"x,y", Delimiter::Char(','));
        assert_eq!(parsed, rows(&[&["12\"x", "y"]]));
    }

    #[test]
    fn draws_a_framed_table() {
        let lines = render_table(&rows(&[&["a", "bb"], &["ccc", "d"]]), TableFlags::default());
        assert_eq!(
            lines,
            vec![
                "┌─────┬────┐",
                "│ a   │ bb │",
                "│ ccc │ d  │",
                "└─────┴────┘",
            ]
        );
    }

    #[test]
    fn every_line_is_the_same_width() {
        let lines = render_table(
            &rows(&[&["a", "bb"], &["ccc", "d"], &["e", "ffff"]]),
            TableFlags::default(),
        );
        let widths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged output: {widths:?}"
        );
    }

    #[test]
    fn pads_ragged_rows_to_the_widest() {
        let lines = render_table(&rows(&[&["a"], &["b", "c"]]), TableFlags::default());
        assert_eq!(
            lines,
            vec!["┌───┬───┐", "│ a │   │", "│ b │ c │", "└───┴───┘"]
        );
    }

    #[test]
    fn rules_off_the_header_row() {
        let flags = TableFlags {
            header: true,
            ..TableFlags::default()
        };
        let lines = render_table(&rows(&[&["name", "n"], &["ab", "1"]]), flags);
        // Colour is disabled under test, so the heading is plain text.
        assert_eq!(
            lines,
            vec![
                "┌──────┬───┐",
                "│ name │ n │",
                "├──────┼───┤",
                "│ ab   │ 1 │",
                "└──────┴───┘",
            ]
        );
    }

    #[test]
    fn draws_an_ascii_frame_on_request() {
        let flags = TableFlags {
            ascii: true,
            ..TableFlags::default()
        };
        let lines = render_table(&rows(&[&["a"]]), flags);
        assert_eq!(lines, vec!["+---+", "| a |", "+---+"]);
    }

    #[test]
    fn right_aligns_a_column_of_numbers() {
        let lines = render_table(&rows(&[&["a", "5"], &["b", "100"]]), TableFlags::default());
        assert_eq!(lines[1], "│ a │   5 │");
        assert_eq!(lines[2], "│ b │ 100 │");
    }

    #[test]
    fn leaves_mixed_columns_left_aligned() {
        let lines = render_table(&rows(&[&["5"], &["n/a"]]), TableFlags::default());
        assert_eq!(lines[1], "│ 5   │");
    }

    #[test]
    fn a_numeric_heading_does_not_follow_its_column_alignment() {
        let flags = TableFlags {
            header: true,
            ..TableFlags::default()
        };
        // The heading stays left-aligned even though the data below is numeric.
        let lines = render_table(&rows(&[&["count"], &["7"]]), flags);
        assert_eq!(lines[1], "│ count │");
        assert_eq!(lines[3], "│     7 │");
    }

    #[test]
    fn recognizes_negative_and_decimal_numbers() {
        assert!(is_numeric_column(["-1", "2.5", "+3"].into_iter()));
        assert!(is_numeric_column(["1", "", "2"].into_iter()));
    }

    #[test]
    fn does_not_treat_words_or_infinities_as_numbers() {
        assert!(!is_numeric_column(["1", "two"].into_iter()));
        assert!(!is_numeric_column(["inf"].into_iter()));
        assert!(!is_numeric_column(["NaN"].into_iter()));
        // A column with nothing in it has no numbers to align.
        assert!(!is_numeric_column(["", ""].into_iter()));
    }

    #[test]
    fn renders_nothing_for_no_rows() {
        assert!(render_table(&[], TableFlags::default()).is_empty());
    }

    #[test]
    fn handles_wide_characters_without_breaking_the_frame() {
        let lines = render_table(&rows(&[&["héllo"], &["x"]]), TableFlags::default());
        let widths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }
}
