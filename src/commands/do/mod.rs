//! `orbital do` — run an orbital command described in plain language.
//!
//! A 45M-parameter Needle 2 model, embedded in the binary, turns the request
//! into a tool call and this module hands that call to the real command, so
//! `orbital do show my ip` renders exactly like `orbital ip`.
//!
//! Nothing here talks to a network or a service: the model runs locally, on one
//! thread, and the whole round trip is a couple of seconds.
//!
//! The call is **grammar-constrained during decoding** (see
//! [`crate::core::needle::grammar`]), not validated afterwards, so the tool
//! name is always one this build declares and its arguments are always the
//! right shape. What the model can still get wrong is *intent* — picking the
//! wrong tool, or inventing an argument the request never gave — so the
//! resolved command line is always printed before it runs.

pub mod tools;

use tools::{has_flag, resolve, resolve_request, Resolved, TOOLS_JSON};

use crate::commands::{find_command, Command, Ctx, Stdin};
use crate::core::needle::grammar::compile;
use crate::core::needle::model::Model;
use crate::core::needle::session::{Generated, Session};
use crate::core::version::BIN;
use crate::style;
use crate::term::{self, Frame, Key, RawMode};
use crate::Res;

/// The weights, fetched and checksummed by `build.rs` and embedded here.
///
/// 13.7 MB of `.cact` v3, read in place straight out of the executable's
/// read-only data — there is no model file to find, download or cache.
const WEIGHTS: &[u8] = include_bytes!("../../../model/needle2.cact");

pub const COMMAND: Command = Command {
    name: "do",
    description: "Run a command described in plain language (e.g. orbital do show my ip)",
    aliases: &["ask"],
    run: None,
    view,
    stdin: Stdin::WhenNoArgs,
};

/// Cap on generated tokens. With the reasoning block skipped a call is ~15
/// tokens, so this only bounds a runaway.
const MAX_NEW: usize = 96;

/// Width of the priming progress bar.
const BAR_WIDTH: usize = 24;

fn view(ctx: &Ctx) -> Res {
    let request = match resolve_request(ctx.args, ctx.input) {
        Some(request) => request,
        // Launched from the menu there are no arguments to work from, so ask.
        None if ctx.interactive => match prompt_for_request()? {
            Some(request) => request,
            None => return Ok(()),
        },
        None => {
            term::emit(&usage_lines());
            return Ok(());
        }
    };

    let generated = match think(&request, ctx.interactive) {
        Ok(generated) => generated,
        Err(message) => {
            term::emit(&[style::red(&message)]);
            return Ok(());
        }
    };

    // No call at all is the model's refusal — it emits the empty call `[]` when
    // nothing in the schema matches, which is the correct answer to "what is
    // the capital of france".
    let Some(call) = generated.calls.first() else {
        term::emit(&no_match_lines(&generated));
        return Ok(());
    };
    let Some(resolved) = resolve(call, &request) else {
        term::emit(&[
            style::red(&format!(
                "Resolved to `{}`, which this build cannot run.",
                call.name
            )),
            style::dim("This is a bug: the schema and the dispatcher have drifted apart."),
        ]);
        return Ok(());
    };

    if has_flag(ctx.args, &["--dry-run", "-n"]) {
        // Command line on stdout so it stays pipeable; what the model thought
        // of its own answer on stderr, where it can't contaminate that.
        term::emit(&[resolved.command_line()]);
        eprintln!("{}", style::dim(&diagnostic_line(&generated)));
        return Ok(());
    }

    term::emit(&[style::dim(&format!("→ {}", resolved.command_line()))]);

    // `serve` publishes the working directory to the local network. That is a
    // fine thing to ask for, but not a fine thing to start because a 45M model
    // guessed it, so it is confirmed first.
    if resolved.is_outward_facing()
        && !has_flag(ctx.args, &["--yes", "-y"])
        && !confirm(&resolved, ctx.interactive)?
    {
        return Ok(());
    }

    let Some(command) = find_command(resolved.command) else {
        term::emit(&[style::red(&format!(
            "Unknown command: {}",
            resolved.command
        ))]);
        return Ok(());
    };
    // Hand off to the real command so `do` renders exactly like it. The piped
    // input, if any, was the request itself and must not be passed along.
    let sub = Ctx {
        args: &resolved.args,
        input: None,
        interactive: ctx.interactive,
    };
    if let Some(plain) = command.run {
        match plain(&sub) {
            Ok(Some(out)) => {
                println!("{out}");
                return Ok(());
            }
            Ok(None) => {}
            Err(message) => {
                eprintln!("{message}");
                return Ok(());
            }
        }
    }
    (command.view)(&sub)
}

/// Ask for the request in place, for the menu and for a bare `orbital do` at a
/// terminal. Returns `None` if the user backed out.
fn prompt_for_request() -> Res<Option<String>> {
    let _raw = RawMode::enable()?;
    let mut frame = Frame::new();
    let mut buffer = String::new();

    loop {
        frame.draw(&prompt_lines(&buffer))?;
        match term::read_key()? {
            Key::Escape | Key::Interrupt => {
                frame.clear()?;
                return Ok(None);
            }
            Key::Enter if !buffer.trim().is_empty() => {
                frame.clear()?;
                return Ok(Some(buffer.trim().to_string()));
            }
            Key::Backspace => {
                buffer.pop();
            }
            Key::Char(c) => buffer.push(c),
            _ => {}
        }
    }
}

fn prompt_lines(buffer: &str) -> Vec<String> {
    let mut lines = vec![
        style::bold_cyan("What do you want to do?"),
        style::dim("Enter to run · Esc to quit"),
        String::new(),
        format!("{}{}{}", style::green("❯ "), buffer, style::green("▏")),
    ];
    if buffer.is_empty() {
        lines.push(style::dim("  e.g. show a qr code for hello"));
    }
    lines
}

/// Load the model, prime the schema prefix, and decode one call.
///
/// Priming is the slow part — the schema is ~180 tokens and the model runs at
/// tens of tokens a second on one thread — so it draws a progress bar when
/// there is a terminal to draw on.
fn think(request: &str, interactive: bool) -> Result<Generated, String> {
    let model = Model::new(WEIGHTS).map_err(|e| format!("Could not load the model: {e}"))?;
    let mut session =
        Session::new(model, TOOLS_JSON).map_err(|e| format!("Could not compile the tools: {e}"))?;

    let mut frame = Frame::new();
    if interactive {
        {
            let mut draw = |done: usize, total: usize| {
                let _ = frame.draw(&[progress_line(done, total)]);
            };
            session.prime(Some(&mut draw));
        }
        let _ = frame.draw(&[style::dim("Deciding…")]);
    } else {
        session.prime(None);
    }

    // no_think forces an empty reasoning block: verified against the reference
    // engine to produce identical calls at roughly half the generated tokens.
    let generated = session
        .generate(request, MAX_NEW, true, None)
        .map_err(|e| format!("Generation failed: {e}"));
    let _ = frame.clear();
    generated
}

fn progress_line(done: usize, total: usize) -> String {
    // A zero total means there was nothing to prime, which reads as done.
    let filled = (done * BAR_WIDTH)
        .checked_div(total)
        .unwrap_or(BAR_WIDTH)
        .min(BAR_WIDTH);
    format!(
        "{} {}{}",
        style::dim("Thinking"),
        style::green(&"━".repeat(filled)),
        style::dim(&"─".repeat(BAR_WIDTH - filled))
    )
}

/// Ask before starting something that reaches off this machine.
fn confirm(resolved: &Resolved, interactive: bool) -> Res<bool> {
    if !interactive {
        term::emit(&[
            style::yellow("Not started: this publishes the current folder to your network."),
            style::dim(&format!(
                "Run it yourself, or re-run with --yes:  {}",
                resolved.command_line()
            )),
        ]);
        return Ok(false);
    }

    let mut frame = Frame::new();
    let _raw = RawMode::enable()?;
    frame.draw(&[format!(
        "{} {}",
        style::yellow("Share the current folder on your network?"),
        style::dim("[y/N]")
    )])?;
    let key = term::read_key()?;
    frame.clear()?;
    Ok(matches!(key, Key::Char('y') | Key::Char('Y')))
}

/// What the model made of its own answer.
///
/// Confidence is a calibrated *groundedness* score — how well the arguments are
/// evidenced in the request, not whether the call is right. A correct call that
/// needed an invented argument scores low, which is exactly what
/// [`tools::grounded_number`] keys off.
fn diagnostic_line(generated: &Generated) -> String {
    let confidence = if generated.confidence < 0.0 {
        "n/a".to_string()
    } else {
        format!("{:.2}", generated.confidence)
    };
    format!("groundedness {confidence} · {} tokens", generated.tokens)
}

fn no_match_lines(generated: &Generated) -> Vec<String> {
    let mut lines = vec![style::yellow("Nothing here matches that.")];
    if generated.truncated {
        lines.push(style::dim(&format!(
            "The model ran out of room before finishing a call: {}",
            generated.text.trim()
        )));
    }
    lines.push(String::new());
    lines.extend(reachable_lines());
    lines
}

/// The commands `do` can reach, read back out of the schema the model was
/// given so the help can never drift from what it can actually call.
fn reachable_lines() -> Vec<String> {
    let mut lines = vec![style::dim("It can reach:")];
    let Ok(grammar) = compile(TOOLS_JSON) else {
        return lines;
    };
    for tool in &grammar.tools {
        let description = find_command(&tool.name)
            .map(|c| c.description)
            .unwrap_or_default();
        lines.push(format!(
            "  {}{}",
            style::cyan(&style::pad(&tool.name, 10)),
            style::dim(description)
        ));
    }
    lines
}

fn usage_lines() -> Vec<String> {
    let mut lines = vec![
        style::yellow("Nothing to do."),
        style::dim(&format!("Usage: {BIN} do <what you want>")),
        style::dim(&format!("   or: echo \"what you want\" | {BIN} do")),
        String::new(),
        style::dim(&format!("  {BIN} do show a qr code for hello")),
        style::dim(&format!("  {BIN} do what is my public ip")),
        String::new(),
        style::dim("  -n, --dry-run   Print the command it resolves to, don't run it"),
        style::dim("  -y, --yes       Skip the confirmation for commands that use the network"),
        String::new(),
    ];
    lines.extend(reachable_lines());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_weights_are_a_cact_blob() {
        // Guards against build.rs writing a truncated or wrong file: the magic
        // tag is the first little-endian word of the v3 header.
        assert!(WEIGHTS.len() > 1_000_000, "weights look truncated");
        let tag = u32::from_le_bytes([WEIGHTS[0], WEIGHTS[1], WEIGHTS[2], WEIGHTS[3]]);
        assert_eq!(tag, 0x05E1_2A83, "not a .cact v3 blob");
    }

    #[test]
    fn the_embedded_model_opens_and_carries_a_tokenizer() {
        let model = Model::new(WEIGHTS).expect("embedded model should open");
        assert_eq!(model.d_model, 512);
        assert_eq!(model.n_layers, 27);
        assert_eq!(model.vocab, 8192);
        assert_eq!(model.window, 256);
        // A real tokenizer, not an empty one: the chat markers must decode.
        assert!(!model.tok.encode_ex("hello", false).is_empty());
    }

    #[test]
    fn the_progress_bar_fills_from_empty_to_full() {
        assert!(progress_line(0, 10).contains("──────"));
        let full = progress_line(10, 10);
        assert!(full.contains("━━━━━━"));
        assert!(!full.contains('─'));
        // A zero total (nothing to prime) reads as done, not as a divide by zero.
        assert!(!progress_line(0, 0).contains('─'));
    }

    #[test]
    fn the_prompt_shows_an_example_only_while_empty() {
        let empty = prompt_lines("").join("\n");
        assert!(empty.contains("What do you want to do?"));
        assert!(empty.contains("e.g. show a qr code for hello"));
        let typed = prompt_lines("show my ip").join("\n");
        assert!(typed.contains("show my ip"));
        assert!(!typed.contains("e.g."));
    }

    #[test]
    fn usage_lists_every_reachable_command() {
        let usage = usage_lines().join("\n");
        for name in ["qr", "ip", "sysinfo", "serve"] {
            assert!(usage.contains(name), "{name} missing from usage");
        }
        assert!(usage.contains("orbital do <what you want>"));
        assert!(usage.contains("--dry-run"));
    }

    #[test]
    fn a_refusal_explains_what_is_reachable() {
        let generated = Generated {
            text: "<tool_call>[]</tool_call>".to_string(),
            calls: Vec::new(),
            confidence: 0.15,
            tokens: 5,
            truncated: false,
        };
        let lines = no_match_lines(&generated).join("\n");
        assert!(lines.contains("Nothing here matches"));
        assert!(lines.contains("sysinfo"));
        assert!(!lines.contains("ran out of room"));
    }

    #[test]
    fn the_diagnostic_reports_groundedness_and_length() {
        let generated = Generated {
            text: String::new(),
            calls: Vec::new(),
            confidence: 0.9771,
            tokens: 14,
            truncated: false,
        };
        assert_eq!(diagnostic_line(&generated), "groundedness 0.98 · 14 tokens");
        // A blob with no confidence head reports nothing rather than -1.
        let headless = Generated {
            confidence: -1.0,
            ..generated
        };
        assert!(diagnostic_line(&headless).contains("n/a"));
    }

    #[test]
    fn a_truncated_generation_says_so() {
        let generated = Generated {
            text: String::new(),
            calls: Vec::new(),
            confidence: -1.0,
            tokens: 96,
            truncated: true,
        };
        assert!(no_match_lines(&generated)
            .join("\n")
            .contains("ran out of room"));
    }
}
