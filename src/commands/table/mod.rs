//! `orbital table` — draw delimited text from a pipe as an aligned table.

pub mod tabulate;

use tabulate::{parse_rows, parse_table_flags, render_table};

use crate::commands::{Command, Ctx, Stdin};
use crate::core::version::BIN;
use crate::style;
use crate::term;
use crate::Res;

pub const COMMAND: Command = Command {
    name: "table",
    description: "Tabulate piped text — whitespace by default, --csv/--tsv/-d",
    aliases: &["tbl", "tabulate"],
    run: None,
    view,
    // The arguments here are options, not data, so the pipe is always read.
    stdin: Stdin::Always,
};

fn view(ctx: &Ctx) -> Res {
    let flags = parse_table_flags(ctx.args);

    let input = match ctx.input {
        Some(input) => input,
        // Nothing was piped in: stdin is a terminal, so there is nothing to
        // wait for and the useful thing to show is how to feed it.
        None => {
            term::emit(&usage_lines());
            return Ok(());
        }
    };

    let rows = parse_rows(input, flags.delimiter);
    if rows.is_empty() {
        term::emit(&[style::yellow("No data to tabulate.")]);
        return Ok(());
    }

    term::emit(&render_table(&rows, flags));
    Ok(())
}

fn usage_lines() -> Vec<String> {
    vec![
        style::yellow("Nothing piped in."),
        style::dim(&format!("Usage: <command> | {BIN} table")),
        style::dim(&format!("   or: {BIN} table --csv < data.csv")),
        String::new(),
        style::dim("Flags: --csv · --tsv · -d <char> · --header · --ascii"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: Option<&str>, args: &[&str]) -> Vec<String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let flags = parse_table_flags(&args);
        match input {
            None => usage_lines(),
            Some(text) => {
                let rows = parse_rows(text, flags.delimiter);
                if rows.is_empty() {
                    vec![style::yellow("No data to tabulate.")]
                } else {
                    render_table(&rows, flags)
                }
            }
        }
    }

    #[test]
    fn tabulates_whitespace_columns() {
        let lines = run(Some("alice 30\nbob 4"), &[]);
        assert_eq!(
            lines,
            vec![
                "┌───────┬────┐",
                "│ alice │ 30 │",
                "│ bob   │  4 │",
                "└───────┴────┘",
            ]
        );
    }

    #[test]
    fn tabulates_csv_with_a_header() {
        let lines = run(Some("name,age\nalice,30"), &["--csv", "--header"]);
        assert!(lines[1].contains("name"));
        assert!(lines[2].starts_with('├'), "expected a header rule");
        assert!(lines[3].contains("alice"));
    }

    #[test]
    fn says_so_when_the_pipe_was_empty() {
        assert_eq!(run(Some("\n  \n"), &[]), vec!["No data to tabulate."]);
    }

    #[test]
    fn explains_itself_when_nothing_was_piped() {
        let usage = run(None, &[]).join("\n");
        assert!(usage.contains("| orbital table"));
        assert!(usage.contains("--csv"));
    }

    #[test]
    fn flags_do_not_become_table_data() {
        // The pipe is the only data source; `--csv` must not appear as a cell.
        let lines = run(Some("a,b"), &["--csv"]).join("\n");
        assert!(!lines.contains("--csv"));
        assert!(lines.contains("a"));
    }
}
