//! Tool-call extraction from generated text.
//!
//! The call was produced under grammar constraint, so this parser only has to
//! handle the shapes the machine can emit: an array of objects with a `name`
//! string and an `arguments` object of string/number/boolean values.

use std::collections::BTreeMap;

use super::grammar::Jr;

/// One argument value. The grammar admits nothing else.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{s}"),
            // Whole numbers print without a trailing `.0`, matching how the
            // model emitted them.
            Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => write!(f, "{}", *n as i64),
            Value::Num(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// One grammar-guaranteed tool call.
///
/// Because generation was constrained, fields are present, correctly typed, and
/// within any declared minimum/maximum — no validation is needed.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub name: String,
    /// Sorted so rendering and tests are deterministic.
    pub args: BTreeMap<String, Value>,
}

impl Call {
    pub fn str_arg(&self, key: &str) -> Option<&str> {
        match self.args.get(key) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn num_arg(&self, key: &str) -> Option<f64> {
        match self.args.get(key) {
            Some(Value::Num(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn bool_arg(&self, key: &str) -> Option<bool> {
        match self.args.get(key) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }
}

/// Extract the tool calls from generated text. An empty vector means either no
/// `<tool_call>` block or the empty call `[]` — in both cases, nothing to run.
pub fn parse_calls(text: &str) -> Vec<Call> {
    let Some(i) = text.find("<tool_call>") else {
        return Vec::new();
    };
    let mut body = &text[i + "<tool_call>".len()..];
    if let Some(e) = body.find("</tool_call>") {
        body = &body[..e];
    }

    let mut j = Jr::new(body.as_bytes());
    if !j.eat(b'[') {
        return Vec::new();
    }
    let mut calls = Vec::new();
    if j.eat(b']') {
        return calls; // the empty call: nothing matched
    }
    while let Some(c) = parse_call(&mut j) {
        calls.push(c);
        if !j.eat(b',') {
            break;
        }
    }
    calls
}

fn parse_call(j: &mut Jr) -> Option<Call> {
    if !j.eat(b'{') {
        return None;
    }
    let mut name = String::new();
    let mut args = None;
    loop {
        let mut key = String::new();
        if !j.jstring(Some(&mut key), 0) || !j.eat(b':') {
            return None;
        }
        match key.as_str() {
            "name" => {
                if !j.jstring(Some(&mut name), 0) {
                    return None;
                }
            }
            "arguments" => args = Some(parse_args(j)?),
            _ => {
                if !j.jskip() {
                    return None;
                }
            }
        }
        if j.eat(b',') {
            continue;
        }
        if j.eat(b'}') {
            break;
        }
        return None;
    }
    Some(Call {
        name,
        args: args.unwrap_or_default(),
    })
}

fn parse_args(j: &mut Jr) -> Option<BTreeMap<String, Value>> {
    let mut args = BTreeMap::new();
    if !j.eat(b'{') {
        return None;
    }
    if j.eat(b'}') {
        return Some(args);
    }
    loop {
        let mut key = String::new();
        if !j.jstring(Some(&mut key), 0) || !j.eat(b':') {
            return None;
        }
        let value = match j.peek()? {
            b'"' => {
                let mut v = String::new();
                if !j.jstring(Some(&mut v), 0) {
                    return None;
                }
                Value::Str(v)
            }
            b't' => {
                if !j.s[j.p..].starts_with(b"true") {
                    return None;
                }
                j.p += 4;
                Value::Bool(true)
            }
            b'f' => {
                if !j.s[j.p..].starts_with(b"false") {
                    return None;
                }
                j.p += 5;
                Value::Bool(false)
            }
            _ => {
                let mut v = 0f64;
                if !j.jnumber(Some(&mut v)) {
                    return None;
                }
                Value::Num(v)
            }
        };
        args.insert(key, value);
        if j.eat(b',') {
            continue;
        }
        if j.eat(b'}') {
            break;
        }
        return None;
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_call_with_every_value_type() {
        let calls = parse_calls(
            r#"<think>x</think><tool_call>[{"name":"ip","arguments":{"public":true,"n":2.5,"s":"hi"}}]</tool_call>"#,
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "ip");
        assert_eq!(calls[0].bool_arg("public"), Some(true));
        assert_eq!(calls[0].num_arg("n"), Some(2.5));
        assert_eq!(calls[0].str_arg("s"), Some("hi"));
    }

    #[test]
    fn the_empty_call_parses_to_no_calls() {
        assert!(parse_calls("<tool_call>[]</tool_call>").is_empty());
    }

    #[test]
    fn text_without_a_call_block_parses_to_no_calls() {
        assert!(parse_calls("just some reasoning").is_empty());
    }

    #[test]
    fn parses_two_calls() {
        let calls = parse_calls(
            r#"<tool_call>[{"name":"a","arguments":{}},{"name":"b","arguments":{"x":1}}]</tool_call>"#,
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "a");
        assert!(calls[0].args.is_empty());
        assert_eq!(calls[1].num_arg("x"), Some(1.0));
    }

    #[test]
    fn an_unterminated_block_still_yields_what_completed() {
        let calls = parse_calls(r#"<tool_call>[{"name":"qr","arguments":{"text":"hi"}}]"#);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].str_arg("text"), Some("hi"));
    }

    #[test]
    fn whole_numbers_render_without_a_decimal_point() {
        assert_eq!(Value::Num(8080.0).to_string(), "8080");
        assert_eq!(Value::Num(2.5).to_string(), "2.5");
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(Value::Str("x".into()).to_string(), "x");
    }
}
