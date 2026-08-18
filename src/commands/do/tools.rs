//! The tool schema `orbital do` exposes to the model, and the mapping from a
//! generated call back onto a real orbital command.
//!
//! Everything here is a pure function over plain data, so the dispatch rules
//! are testable without loading a 13.7 MB model.

use crate::core::needle::parse::Call;

/// The tools the model may call, as a JSON schema.
///
/// **Four tools is the ceiling, not a preference.** The prompt prefix shares a
/// 256-token sliding window with the request, and this schema already costs
/// 182 of it. Measured against the reference engine: three tools answered 7/7
/// test queries, four answered 9/11, and five began firing `sysinfo` at
/// unrelated questions like "what is the capital of france". Adding a fifth
/// tool degrades the four that are here — the upstream design solves this with
/// a tool-retrieval head that neither this port nor the C reference implements.
///
/// **Descriptions are load-bearing**, not documentation: the wording is what
/// makes "how much memory do i have" reach `sysinfo`. Treat them as a tuning
/// knob and re-measure after any edit. Indentation is irrelevant — the session
/// compacts this before the model sees it.
pub const TOOLS_JSON: &str = concat!(
    r#"[{"name":"qr","description":"Encode text into a QR code","#,
    r#""parameters":{"type":"object","properties":{"#,
    r#""text":{"type":"string","description":"the text to encode"}},"#,
    r#""required":["text"]}},"#,
    r#"{"name":"ip","description":"Show this machine's IP address","#,
    r#""parameters":{"type":"object","properties":{"#,
    r#""public":{"type":"boolean","description":"public internet address, not local"}}}},"#,
    r#"{"name":"sysinfo","description":"Show cpu, memory, disk and os details","#,
    r#""parameters":{"type":"object","properties":{}}},"#,
    r#"{"name":"serve","description":"Share the current folder over HTTP","#,
    r#""parameters":{"type":"object","properties":{"#,
    r#""port":{"type":"integer","description":"port number, only if one is named","#,
    r#""minimum":1024,"maximum":65535}}}}]"#
);

/// An orbital command to run, resolved from a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The command name, as it appears in the registry.
    pub command: &'static str,
    /// Arguments to hand it, exactly as a CLI invocation would.
    pub args: Vec<String>,
}

impl Resolved {
    /// The equivalent command line, for showing the user what was run.
    pub fn command_line(&self) -> String {
        let mut line = format!("orbital {}", self.command);
        for arg in &self.args {
            // Quote anything a shell would otherwise split or interpret.
            if arg.is_empty() || arg.contains(|c: char| c.is_whitespace() || "'\"\\$`".contains(c))
            {
                line.push_str(&format!(" '{}'", arg.replace('\'', r"'\''")));
            } else {
                line.push(' ');
                line.push_str(arg);
            }
        }
        line
    }

    /// Whether running this reaches beyond the machine. `serve` binds every
    /// interface and publishes the working directory to the local network, so
    /// it is confirmed before it runs rather than started on a guess.
    pub fn is_outward_facing(&self) -> bool {
        self.command == "serve"
    }
}

/// Whether a number the model produced is actually evidenced in the request.
///
/// The model fills optional numeric fields whether or not the request supplies
/// a value — "share this folder on the web" reliably invents a port. Its own
/// confidence head catches this (0.002 for an invented port against 0.998 for
/// one that was asked for), so the same test is applied here directly: keep the
/// value only when its digits appear in what the user typed.
///
/// Deliberately literal — "port nine thousand" is not recognised, and treating
/// it as absent falls back to the command's documented default.
pub fn grounded_number(query: &str, n: i64) -> bool {
    query.contains(&n.to_string())
}

/// Map one grammar-guaranteed call onto an orbital command.
///
/// Returns `None` for a call this build cannot dispatch. The grammar can only
/// emit declared tool names, so that means the schema and this function have
/// drifted apart — a bug, not user input.
pub fn resolve(call: &Call, query: &str) -> Option<Resolved> {
    let args = match call.name.as_str() {
        "qr" => vec![call.str_arg("text")?.to_string()],
        "ip" => {
            if call.bool_arg("public") == Some(true) {
                vec!["--public".to_string()]
            } else {
                Vec::new()
            }
        }
        "sysinfo" => Vec::new(),
        "serve" => match call.num_arg("port") {
            Some(p) if grounded_number(query, p as i64) => vec![(p as i64).to_string()],
            _ => Vec::new(),
        },
        _ => return None,
    };
    let command = match call.name.as_str() {
        "qr" => "qr",
        "ip" => "ip",
        "sysinfo" => "sysinfo",
        "serve" => "serve",
        _ => return None,
    };
    Some(Resolved { command, args })
}

/// Flags `do` consumes itself, which must not become part of the request text.
const OWN_FLAGS: &[&str] = &["--dry-run", "-n", "--yes", "-y"];

pub fn has_flag(args: &[String], names: &[&str]) -> bool {
    args.iter().any(|a| names.contains(&a.as_str()))
}

/// The request text: the positional arguments joined, or piped stdin.
///
/// Arguments win over the pipe, matching `qr`: `orbital do show my ip` is the
/// payload, not an option.
pub fn resolve_request(args: &[String], input: Option<&str>) -> Option<String> {
    let joined = args
        .iter()
        .filter(|a| !OWN_FLAGS.contains(&a.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let joined = joined.trim();
    if !joined.is_empty() {
        return Some(joined.to_string());
    }
    let piped = input?.trim();
    if piped.is_empty() {
        None
    } else {
        Some(piped.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::needle::grammar::compile;
    use crate::core::needle::parse::{parse_calls, Value};
    use std::collections::BTreeMap;

    fn call(name: &str, args: &[(&str, Value)]) -> Call {
        Call {
            name: name.to_string(),
            args: args
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_shipped_schema_compiles() {
        let g = compile(TOOLS_JSON).unwrap();
        assert_eq!(g.tools.len(), 4);
        let names: Vec<&str> = g.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["qr", "ip", "sysinfo", "serve"]);
    }

    #[test]
    fn every_tool_in_the_schema_names_a_real_command() {
        for tool in compile(TOOLS_JSON).unwrap().tools {
            assert!(
                crate::commands::find_command(&tool.name).is_some(),
                "schema declares {}, which is not a registered command",
                tool.name
            );
        }
    }

    #[test]
    fn every_tool_in_the_schema_can_be_dispatched() {
        // A tool the grammar can emit but `resolve` does not know would be a
        // silent dead end, so pin the two lists together.
        for tool in compile(TOOLS_JSON).unwrap().tools {
            let stub = call(&tool.name, &[("text", Value::Str("x".into()))]);
            assert!(
                resolve(&stub, "x").is_some(),
                "{} is declared but not dispatched",
                tool.name
            );
        }
    }

    #[test]
    fn resolves_a_qr_call_to_its_payload() {
        let c = call("qr", &[("text", Value::Str("hello world".into()))]);
        let r = resolve(&c, "qr code for hello world").unwrap();
        assert_eq!(r.command, "qr");
        assert_eq!(r.args, strings(&["hello world"]));
    }

    #[test]
    fn resolves_the_public_ip_flag_only_when_asked() {
        let public = call("ip", &[("public", Value::Bool(true))]);
        assert_eq!(
            resolve(&public, "my public ip").unwrap().args,
            strings(&["--public"])
        );
        // The model emits `public=false` unprompted; that is just the default.
        let local = call("ip", &[("public", Value::Bool(false))]);
        assert!(resolve(&local, "my ip").unwrap().args.is_empty());
        assert!(resolve(&call("ip", &[]), "my ip").unwrap().args.is_empty());
    }

    #[test]
    fn keeps_a_port_the_request_actually_names() {
        let c = call("serve", &[("port", Value::Num(9000.0))]);
        let r = resolve(&c, "serve the current directory on port 9000").unwrap();
        assert_eq!(r.args, strings(&["9000"]));
    }

    #[test]
    fn drops_a_port_the_model_invented() {
        // Measured behaviour: "share this folder on the web" produces a port
        // out of nowhere. Falling back to no argument uses orbital's default.
        let c = call("serve", &[("port", Value::Num(12024.0))]);
        let r = resolve(&c, "share this folder on the web").unwrap();
        assert!(r.args.is_empty(), "invented port should be dropped");
    }

    #[test]
    fn grounding_matches_only_digits_present_in_the_request() {
        assert!(grounded_number("serve on port 8080", 8080));
        assert!(!grounded_number("serve this folder", 8080));
        // A substring hit is enough — the point is evidence, not parsing.
        assert!(grounded_number("call 18080 now", 8080));
    }

    #[test]
    fn only_serve_is_treated_as_outward_facing() {
        for (command, outward) in [
            ("qr", false),
            ("ip", false),
            ("sysinfo", false),
            ("serve", true),
        ] {
            let r = Resolved {
                command,
                args: Vec::new(),
            };
            assert_eq!(r.is_outward_facing(), outward, "{command}");
        }
    }

    #[test]
    fn renders_a_runnable_command_line() {
        let r = Resolved {
            command: "qr",
            args: strings(&["hello"]),
        };
        assert_eq!(r.command_line(), "orbital qr hello");
    }

    #[test]
    fn quotes_arguments_a_shell_would_split() {
        let r = Resolved {
            command: "qr",
            args: strings(&["hello world"]),
        };
        assert_eq!(r.command_line(), "orbital qr 'hello world'");
        let quoted = Resolved {
            command: "qr",
            args: strings(&["it's"]),
        };
        assert_eq!(quoted.command_line(), r"orbital qr 'it'\''s'");
    }

    #[test]
    fn takes_the_request_from_the_arguments() {
        let args = strings(&["show", "qr", "code", "for", "test"]);
        assert_eq!(
            resolve_request(&args, None).as_deref(),
            Some("show qr code for test")
        );
    }

    #[test]
    fn strips_its_own_flags_from_the_request() {
        let args = strings(&["--dry-run", "what", "is", "my", "ip"]);
        assert_eq!(
            resolve_request(&args, None).as_deref(),
            Some("what is my ip")
        );
        assert!(has_flag(&args, &["--dry-run", "-n"]));
    }

    #[test]
    fn falls_back_to_piped_input() {
        assert_eq!(
            resolve_request(&[], Some("  what is my ip\n")).as_deref(),
            Some("what is my ip")
        );
        assert!(resolve_request(&[], Some("   \n")).is_none());
        assert!(resolve_request(&[], None).is_none());
    }

    #[test]
    fn arguments_win_over_the_pipe() {
        let args = strings(&["my", "ip"]);
        assert_eq!(
            resolve_request(&args, Some("piped")).as_deref(),
            Some("my ip")
        );
    }

    #[test]
    fn resolves_a_call_parsed_straight_out_of_generated_text() {
        let calls =
            parse_calls(r#"<tool_call>[{"name":"qr","arguments":{"text":"test"}}]</tool_call>"#);
        let r = resolve(&calls[0], "show qr code for 'test'").unwrap();
        assert_eq!(r.command_line(), "orbital qr test");
    }
}
