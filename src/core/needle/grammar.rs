//! JSON schema → byte-level grammar, and the state machine that enforces it.
//!
//! The tool call is constrained **during decoding**, not validated afterwards:
//! [`GState::byte`] is fed every byte a candidate token would emit, and a token
//! is only legal if all of its bytes are accepted. So the tool name is always
//! one that was declared, required fields are present, types are correct, and
//! numbers land inside any declared `minimum`/`maximum`.

/// Grammar limits, matching the reference engine.
pub const GR_MAX_TOOLS: usize = 8;
pub const GR_MAX_PROPS: usize = 12;
pub const GR_MAX_ENUM: usize = 16;
/// Names must be <= 39 characters.
pub const GR_STR_LEN: usize = 40;

/// Value types the grammar enforces. Nested objects and arrays are rejected at
/// compile time rather than silently going unenforced, so we never believe we
/// are enforcing a schema we are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VType {
    String,
    Integer,
    Number,
    Boolean,
}

/// One declared property of a tool.
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub vtype: VType,
    pub enums: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Default for Prop {
    fn default() -> Self {
        Prop {
            name: String::new(),
            vtype: VType::String,
            enums: Vec::new(),
            min: None,
            max: None,
        }
    }
}

/// One declared tool.
#[derive(Debug, Clone, Default)]
pub struct Tool {
    pub name: String,
    pub props: Vec<Prop>,
    /// Bitmask over `props`.
    pub required: u16,
}

/// A compiled set of tool schemas.
#[derive(Debug, Clone, Default)]
pub struct Grammar {
    pub tools: Vec<Tool>,
}

/// `(1 << n) - 1`, computed in 32 bits so `n == 16` doesn't overflow the shift.
fn mask(n: usize) -> u16 {
    ((1u32 << n) - 1) as u16
}

// ---------------------------------------------------------- JSON reader

/// A minimal byte-level JSON reader, shared by the schema compiler and the
/// tool-call parser.
pub struct Jr<'a> {
    pub s: &'a [u8],
    pub p: usize,
}

impl<'a> Jr<'a> {
    pub fn new(s: &'a [u8]) -> Self {
        Jr { s, p: 0 }
    }

    pub fn skip_ws(&mut self) {
        while self.p < self.s.len() && matches!(self.s[self.p], b' ' | b'\t' | b'\n' | b'\r') {
            self.p += 1;
        }
    }

    pub fn eat(&mut self, c: u8) -> bool {
        self.skip_ws();
        if self.p < self.s.len() && self.s[self.p] == c {
            self.p += 1;
            return true;
        }
        false
    }

    pub fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s.get(self.p).copied()
    }

    /// Read a JSON string. Handles the escapes a schema realistically contains;
    /// `\u` is rejected rather than mangled. `max_len` of 0 means unlimited.
    pub fn jstring(&mut self, out: Option<&mut String>, max_len: usize) -> bool {
        self.skip_ws();
        if self.p >= self.s.len() || self.s[self.p] != b'"' {
            return false;
        }
        self.p += 1;
        let mut b: Vec<u8> = Vec::new();
        let capture = out.is_some();
        while self.p < self.s.len() && self.s[self.p] != b'"' {
            let mut c = self.s[self.p];
            self.p += 1;
            if c == b'\\' {
                if self.p >= self.s.len() {
                    return false;
                }
                c = self.s[self.p];
                self.p += 1;
                c = match c {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    b'"' | b'\\' | b'/' => c,
                    _ => return false,
                };
            }
            if capture {
                if max_len > 0 && b.len() + 1 >= max_len {
                    return false;
                }
                b.push(c);
            }
        }
        if self.p >= self.s.len() {
            return false;
        }
        self.p += 1;
        if let Some(out) = out {
            *out = String::from_utf8_lossy(&b).into_owned();
        }
        true
    }

    pub fn jnumber(&mut self, out: Option<&mut f64>) -> bool {
        self.skip_ws();
        let start = self.p;
        if self.p < self.s.len() && (self.s[self.p] == b'-' || self.s[self.p] == b'+') {
            self.p += 1;
        }
        while self.p < self.s.len() && (self.s[self.p].is_ascii_digit() || self.s[self.p] == b'.') {
            self.p += 1;
        }
        if self.p < self.s.len() && (self.s[self.p] == b'e' || self.s[self.p] == b'E') {
            self.p += 1;
            if self.p < self.s.len() && (self.s[self.p] == b'-' || self.s[self.p] == b'+') {
                self.p += 1;
            }
            while self.p < self.s.len() && self.s[self.p].is_ascii_digit() {
                self.p += 1;
            }
        }
        if self.p == start {
            return false;
        }
        let text = String::from_utf8_lossy(&self.s[start..self.p]);
        match text.parse::<f64>() {
            Ok(v) => {
                if let Some(out) = out {
                    *out = v;
                }
                true
            }
            Err(_) => {
                self.p = start;
                false
            }
        }
    }

    /// Skip any value, so unknown schema keys cost nothing.
    pub fn jskip(&mut self) -> bool {
        self.skip_ws();
        if self.p >= self.s.len() {
            return false;
        }
        if self.s[self.p] == b'"' {
            return self.jstring(None, 0);
        }
        if self.s[self.p] == b'{' || self.s[self.p] == b'[' {
            let open = self.s[self.p];
            let close = if open == b'[' { b']' } else { b'}' };
            let mut depth = 0i32;
            while self.p < self.s.len() {
                if self.s[self.p] == b'"' {
                    if !self.jstring(None, 0) {
                        return false;
                    }
                    continue;
                }
                if self.s[self.p] == open {
                    depth += 1;
                } else if self.s[self.p] == close {
                    depth -= 1;
                    if depth == 0 {
                        self.p += 1;
                        return true;
                    }
                }
                self.p += 1;
            }
            return false;
        }
        while self.p < self.s.len() && !matches!(self.s[self.p], b',' | b'}' | b']') {
            self.p += 1;
        }
        true
    }
}

/// Strip whitespace outside string literals.
///
/// This matters more than it looks. The model was trained on schemas rendered
/// with `json.dumps(separators=(",",":"))`, and a pretty-printed schema is far
/// enough off-distribution that it starts inventing tools that were never
/// declared — with no error anywhere. Compacting on the way in makes the schema
/// text's formatting irrelevant.
pub fn json_compact(src: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let (mut in_str, mut esc) = (false, false);
    for &c in src.as_bytes() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            continue;
        }
        out.push(c);
        if c == b'"' {
            in_str = true;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ------------------------------------------------------------- compiling

fn parse_type(s: &str) -> Option<VType> {
    match s {
        "string" => Some(VType::String),
        "integer" => Some(VType::Integer),
        "number" => Some(VType::Number),
        "boolean" => Some(VType::Boolean),
        _ => None,
    }
}

fn parse_prop(j: &mut Jr, p: &mut Prop) -> Result<(), &'static str> {
    p.vtype = VType::String;
    if !j.eat(b'{') {
        return Err("property is not an object");
    }
    if j.eat(b'}') {
        return Ok(());
    }
    loop {
        let mut key = String::new();
        if !j.jstring(Some(&mut key), 64) {
            return Err("bad property key");
        }
        if !j.eat(b':') {
            return Err("expected ':'");
        }
        match key.as_str() {
            "type" => {
                let mut t = String::new();
                if !j.jstring(Some(&mut t), 32) {
                    return Err("bad type");
                }
                p.vtype =
                    parse_type(&t).ok_or("unsupported type (object/array/null not supported)")?;
            }
            "enum" => {
                if !j.eat(b'[') {
                    return Err("enum is not an array");
                }
                if !j.eat(b']') {
                    loop {
                        if p.enums.len() >= GR_MAX_ENUM {
                            return Err("too many enum values");
                        }
                        let mut v = String::new();
                        if !j.jstring(Some(&mut v), GR_STR_LEN) {
                            return Err("non-string enum value");
                        }
                        p.enums.push(v);
                        if j.eat(b',') {
                            continue;
                        }
                        if j.eat(b']') {
                            break;
                        }
                        return Err("malformed enum");
                    }
                }
            }
            "minimum" => {
                let mut v = 0f64;
                if !j.jnumber(Some(&mut v)) {
                    return Err("bad minimum");
                }
                p.min = Some(v);
            }
            "maximum" => {
                let mut v = 0f64;
                if !j.jnumber(Some(&mut v)) {
                    return Err("bad maximum");
                }
                p.max = Some(v);
            }
            _ => {
                if !j.jskip() {
                    return Err("bad property value");
                }
            }
        }
        if j.eat(b',') {
            continue;
        }
        if j.eat(b'}') {
            break;
        }
        return Err("malformed property");
    }
    Ok(())
}

fn find_prop(t: &Tool, name: &str) -> Option<usize> {
    t.props.iter().position(|p| p.name == name)
}

fn parse_parameters(j: &mut Jr, t: &mut Tool) -> Result<(), &'static str> {
    if !j.eat(b'{') {
        return Err("parameters is not an object");
    }
    if j.eat(b'}') {
        return Ok(());
    }
    loop {
        let mut key = String::new();
        if !j.jstring(Some(&mut key), 64) {
            return Err("bad parameters key");
        }
        if !j.eat(b':') {
            return Err("expected ':'");
        }
        match key.as_str() {
            "properties" => {
                if !j.eat(b'{') {
                    return Err("properties is not an object");
                }
                if !j.eat(b'}') {
                    loop {
                        if t.props.len() >= GR_MAX_PROPS {
                            return Err("too many properties");
                        }
                        let mut p = Prop::default();
                        if !j.jstring(Some(&mut p.name), GR_STR_LEN) {
                            return Err("bad property name");
                        }
                        if !j.eat(b':') {
                            return Err("expected ':'");
                        }
                        parse_prop(j, &mut p)?;
                        t.props.push(p);
                        if j.eat(b',') {
                            continue;
                        }
                        if j.eat(b'}') {
                            break;
                        }
                        return Err("malformed properties");
                    }
                }
            }
            "required" => {
                // Resolved after properties are known; stash and re-scan below.
                let save = j.p;
                if !j.jskip() {
                    return Err("bad required");
                }
                let mut r = Jr { s: j.s, p: save };
                if r.eat(b'[') && !r.eat(b']') {
                    loop {
                        let mut nm = String::new();
                        if !r.jstring(Some(&mut nm), GR_STR_LEN) {
                            break;
                        }
                        if let Some(idx) = find_prop(t, &nm) {
                            t.required |= 1 << idx;
                        }
                        if r.eat(b',') {
                            continue;
                        }
                        break;
                    }
                }
            }
            _ => {
                if !j.jskip() {
                    return Err("bad parameters value");
                }
            }
        }
        if j.eat(b',') {
            continue;
        }
        if j.eat(b'}') {
            break;
        }
        return Err("malformed parameters");
    }
    Ok(())
}

/// Parse a tools JSON array into a grammar.
pub fn compile(tools_json: &str) -> Result<Grammar, &'static str> {
    let mut g = Grammar::default();
    let mut j = Jr::new(tools_json.as_bytes());

    if !j.eat(b'[') {
        return Err("tools is not an array");
    }
    if j.eat(b']') {
        return Ok(g);
    }
    loop {
        if g.tools.len() >= GR_MAX_TOOLS {
            return Err("too many tools");
        }
        let mut t = Tool::default();
        if !j.eat(b'{') {
            return Err("tool is not an object");
        }
        loop {
            let mut key = String::new();
            if !j.jstring(Some(&mut key), 64) {
                return Err("bad tool key");
            }
            if !j.eat(b':') {
                return Err("expected ':'");
            }
            match key.as_str() {
                "name" => {
                    if !j.jstring(Some(&mut t.name), GR_STR_LEN) {
                        return Err("bad tool name");
                    }
                }
                "parameters" => parse_parameters(&mut j, &mut t)?,
                _ => {
                    if !j.jskip() {
                        return Err("bad tool value");
                    }
                }
            }
            if j.eat(b',') {
                continue;
            }
            if j.eat(b'}') {
                break;
            }
            return Err("malformed tool");
        }
        if t.name.is_empty() {
            return Err("tool has no name");
        }
        g.tools.push(t);

        if j.eat(b',') {
            continue;
        }
        if j.eat(b']') {
            break;
        }
        return Err("malformed tools array");
    }
    Ok(g)
}

// --------------------------------------------------------------- matching

/// Parser phases, in the order the machine moves through them. The ordering is
/// load-bearing: everything from [`Phase::ArgsKey`] on has a selected tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Not yet inside `<tool_call>`; reasoning is unconstrained.
    Off,
    ArrayOpen,
    ArrayFirst,
    ObjOpen,
    NameKey,
    NameVal,
    ArgsKey,
    ArgsFirst,
    /// After a ',' inside arguments: only a key may follow.
    NextProp,
    PropName,
    PropColon,
    ValStart,
    ValStr,
    ValEnum,
    ValNum,
    ValBool,
    AfterVal,
    /// The arguments object is closed; expect the call's '}'.
    AfterObj,
    /// The call object is closed; expect ',' or ']'.
    CloseCall,
    Done,
}

/// A plain copyable parser state.
///
/// The sampler copies it once per candidate token to trial-run that token's
/// bytes and throws the copy away, so a rejected token costs no allocation.
/// **Keep every field `Copy`** — adding a `Vec`, map, or owned pointer turns a
/// cheap copy into aliasing and will corrupt the grammar.
#[derive(Clone, Copy)]
pub struct GState<'a> {
    g: &'a Grammar,
    pub phase: Phase,
    /// Selected tool index.
    tool: usize,
    /// Selected property index.
    prop: usize,
    seen: u16,
    cand: u16,
    lit_pos: usize,
    num_frac: bool,
    num_any: bool,
    num_neg: bool,
    num_val: f64,
    num_scale: f64,
    lit: &'static str,
}

impl<'a> GState<'a> {
    /// Begin in the disengaged state.
    pub fn new(g: &'a Grammar) -> Self {
        GState {
            g,
            phase: Phase::Off,
            tool: 0,
            prop: 0,
            seen: 0,
            cand: 0,
            lit_pos: 0,
            num_frac: false,
            num_any: false,
            num_neg: false,
            num_val: 0.0,
            num_scale: 0.1,
            lit: "",
        }
    }

    /// Engage the machine; call when `<tool_call>` has been emitted.
    pub fn open(&mut self) {
        self.phase = Phase::ArrayOpen;
    }

    /// Whether the call is finished and only `</tool_call>` may follow.
    pub fn complete(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Match against a fixed literal, moving to `next` once it is fully read.
    fn lit_byte(&mut self, c: u8, lit: &'static str, next: Phase) -> bool {
        let bytes = lit.as_bytes();
        if self.lit_pos >= bytes.len() || bytes[self.lit_pos] != c {
            return false;
        }
        self.lit_pos += 1;
        if self.lit_pos == bytes.len() {
            self.lit_pos = 0;
            self.phase = next;
        }
        true
    }

    /// Narrow a set of alternatives by one byte.
    fn alt_byte(cand: &mut u16, pos: usize, c: u8, opts: &[String]) -> bool {
        let mut next = 0u16;
        for (i, o) in opts.iter().enumerate() {
            if *cand & (1 << i) == 0 {
                continue;
            }
            if o.as_bytes().get(pos) == Some(&c) {
                next |= 1 << i;
            }
        }
        *cand = next;
        next != 0
    }

    /// Whether some still-live alternative ends exactly at `pos`.
    fn alt_finished(cand: u16, pos: usize, opts: &[String]) -> bool {
        opts.iter()
            .enumerate()
            .any(|(i, o)| cand & (1 << i) != 0 && pos == o.len())
    }

    /// Whether the number built so far can still reach a valid value. While
    /// digits are arriving only the upper bound can be violated irrecoverably,
    /// because appending digits only grows magnitude.
    fn num_in_range(&self, p: &Prop, final_check: bool) -> bool {
        let v = if self.num_neg {
            -self.num_val
        } else {
            self.num_val
        };
        if let Some(max) = p.max {
            if !self.num_neg && v > max {
                return false;
            }
        }
        if let Some(min) = p.min {
            if self.num_neg && v < min {
                return false;
            }
        }
        if final_check {
            if p.min.is_some_and(|min| v < min) {
                return false;
            }
            if p.max.is_some_and(|max| v > max) {
                return false;
            }
        }
        true
    }

    /// Feed one byte. Returns `true` if accepted (state advanced), `false` if
    /// rejected — after which the state is undefined, so callers work on a copy.
    pub fn byte(&mut self, c: u8) -> bool {
        match self.phase {
            Phase::Off => true, // reasoning is unconstrained

            Phase::ArrayOpen => {
                if c != b'[' {
                    return false;
                }
                self.phase = Phase::ArrayFirst;
                true
            }

            Phase::ArrayFirst | Phase::ObjOpen => {
                // Only the first position may refuse outright with `[]`.
                if c == b']' && self.phase == Phase::ArrayFirst {
                    self.phase = Phase::Done;
                    return true;
                }
                if c != b'{' {
                    return false;
                }
                self.cand = mask(self.g.tools.len());
                self.lit = "\"name\":\"";
                self.lit_pos = 0;
                self.phase = Phase::NameKey;
                true
            }

            Phase::NameKey => {
                let lit = self.lit;
                if !self.lit_byte(c, lit, Phase::NameVal) {
                    return false;
                }
                if self.phase == Phase::NameVal {
                    self.lit_pos = 0;
                }
                true
            }

            Phase::NameVal => {
                if c == b'"' {
                    // Exactly one still-live tool name can end here.
                    let done = self.g.tools.iter().enumerate().position(|(i, t)| {
                        self.cand & (1 << i) != 0 && t.name.len() == self.lit_pos
                    });
                    let Some(idx) = done else { return false };
                    self.tool = idx;
                    self.seen = 0;
                    self.lit = ",\"arguments\":{";
                    self.lit_pos = 0;
                    self.phase = Phase::ArgsKey;
                    return true;
                }
                let mut next = 0u16;
                for (i, t) in self.g.tools.iter().enumerate() {
                    if self.cand & (1 << i) == 0 {
                        continue;
                    }
                    if t.name.as_bytes().get(self.lit_pos) == Some(&c) {
                        next |= 1 << i;
                    }
                }
                if next == 0 {
                    return false;
                }
                self.cand = next;
                self.lit_pos += 1;
                true
            }

            Phase::ArgsKey => {
                let lit = self.lit;
                self.lit_byte(c, lit, Phase::ArgsFirst)
            }

            Phase::ArgsFirst | Phase::NextProp => {
                let t = &self.g.tools[self.tool];
                // ArgsFirst may close an empty argument object; NextProp
                // follows a comma, so a key is mandatory and a trailing comma
                // is rejected.
                if c == b'}' {
                    if self.phase != Phase::ArgsFirst || t.required & self.seen != t.required {
                        return false;
                    }
                    self.phase = Phase::AfterObj;
                    return true;
                }
                if c != b'"' {
                    return false;
                }
                self.cand = mask(t.props.len()) & !self.seen;
                if self.cand == 0 {
                    return false;
                }
                self.lit_pos = 0;
                self.phase = Phase::PropName;
                true
            }

            Phase::PropName => {
                let t = &self.g.tools[self.tool];
                if c == b'"' {
                    let idx = t.props.iter().enumerate().position(|(i, p)| {
                        self.cand & (1 << i) != 0 && p.name.len() == self.lit_pos
                    });
                    let Some(idx) = idx else { return false };
                    self.prop = idx;
                    self.lit = ":";
                    self.lit_pos = 0;
                    self.phase = Phase::PropColon;
                    return true;
                }
                let mut next = 0u16;
                for (i, p) in t.props.iter().enumerate() {
                    if self.cand & (1 << i) == 0 {
                        continue;
                    }
                    if p.name.as_bytes().get(self.lit_pos) == Some(&c) {
                        next |= 1 << i;
                    }
                }
                if next == 0 {
                    return false;
                }
                self.cand = next;
                self.lit_pos += 1;
                true
            }

            Phase::PropColon => {
                if c != b':' {
                    return false;
                }
                self.phase = Phase::ValStart;
                true
            }

            Phase::ValStart => {
                let p = &self.g.tools[self.tool].props[self.prop];
                match p.vtype {
                    VType::String => {
                        if c != b'"' {
                            return false;
                        }
                        self.lit_pos = 0;
                        if p.enums.is_empty() {
                            self.phase = Phase::ValStr;
                        } else {
                            self.cand = mask(p.enums.len());
                            self.phase = Phase::ValEnum;
                        }
                        true
                    }
                    VType::Boolean => {
                        if c == b't' {
                            self.lit = "true";
                        } else if c == b'f' {
                            self.lit = "false";
                        } else {
                            return false;
                        }
                        self.lit_pos = 1;
                        self.phase = Phase::ValBool;
                        true
                    }
                    VType::Integer | VType::Number => {
                        self.num_val = 0.0;
                        self.num_frac = false;
                        self.num_any = false;
                        self.num_neg = false;
                        self.num_scale = 0.1;
                        if c == b'-' {
                            if p.min.is_some_and(|min| min >= 0.0) {
                                return false;
                            }
                            self.num_neg = true;
                            self.phase = Phase::ValNum;
                            return true;
                        }
                        if !c.is_ascii_digit() {
                            return false;
                        }
                        self.num_val = (c - b'0') as f64;
                        self.num_any = true;
                        if !self.num_in_range(p, false) {
                            return false;
                        }
                        self.phase = Phase::ValNum;
                        true
                    }
                }
            }

            Phase::ValEnum => {
                let p = &self.g.tools[self.tool].props[self.prop];
                if c == b'"' {
                    if !Self::alt_finished(self.cand, self.lit_pos, &p.enums) {
                        return false;
                    }
                    self.seen |= 1 << self.prop;
                    self.phase = Phase::AfterVal;
                    return true;
                }
                let mut cand = self.cand;
                if !Self::alt_byte(&mut cand, self.lit_pos, c, &p.enums) {
                    return false;
                }
                self.cand = cand;
                self.lit_pos += 1;
                true
            }

            Phase::ValStr => {
                // Free-form string: everything but a raw control char or a
                // quote, which terminates. Backslash escapes are not emitted by
                // the model for these fields and are rejected to keep the
                // machine total.
                if c == b'"' {
                    self.seen |= 1 << self.prop;
                    self.phase = Phase::AfterVal;
                    return true;
                }
                if c < 0x20 || c == b'\\' {
                    return false;
                }
                if self.lit_pos < 255 {
                    self.lit_pos += 1;
                }
                true
            }

            Phase::ValBool => {
                let bytes = self.lit.as_bytes();
                if self.lit_pos >= bytes.len() || bytes[self.lit_pos] != c {
                    return false;
                }
                self.lit_pos += 1;
                if self.lit_pos == bytes.len() {
                    self.seen |= 1 << self.prop;
                    self.phase = Phase::AfterVal;
                }
                true
            }

            Phase::ValNum => {
                let t = &self.g.tools[self.tool];
                let p = &t.props[self.prop];
                if c.is_ascii_digit() {
                    if self.num_frac {
                        self.num_val += (c - b'0') as f64 * self.num_scale;
                        self.num_scale *= 0.1;
                    } else {
                        self.num_val = self.num_val * 10.0 + (c - b'0') as f64;
                    }
                    self.num_any = true;
                    return self.num_in_range(p, false);
                }
                if c == b'.' {
                    if p.vtype == VType::Integer || self.num_frac || !self.num_any {
                        return false;
                    }
                    self.num_frac = true;
                    return true;
                }
                // The value ends: only a separator may follow, and the number
                // must be complete and in range.
                if (c == b',' || c == b'}') && self.num_any && self.num_in_range(p, true) {
                    self.seen |= 1 << self.prop;
                    if c == b',' {
                        if mask(t.props.len()) & !self.seen == 0 {
                            return false;
                        }
                        self.phase = Phase::NextProp;
                        return true;
                    }
                    if t.required & self.seen != t.required {
                        return false;
                    }
                    self.phase = Phase::AfterObj;
                    return true;
                }
                false
            }

            Phase::AfterVal => {
                let t = &self.g.tools[self.tool];
                if c == b',' {
                    if mask(t.props.len()) & !self.seen == 0 {
                        return false;
                    }
                    self.phase = Phase::NextProp;
                    return true;
                }
                if c == b'}' {
                    if t.required & self.seen != t.required {
                        return false;
                    }
                    self.phase = Phase::AfterObj;
                    return true;
                }
                false
            }

            // The arguments object is closed; this '}' closes the call object.
            Phase::AfterObj => {
                if c != b'}' {
                    return false;
                }
                self.phase = Phase::CloseCall;
                true
            }

            Phase::CloseCall => {
                if c == b',' {
                    self.phase = Phase::ObjOpen;
                    return true;
                }
                if c == b']' {
                    self.phase = Phase::Done;
                    return true;
                }
                false
            }

            Phase::Done => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the C reference's `nd_gtest` schema exactly, so the two test
    /// suites check the same machine.
    const SCHEMA: &str = concat!(
        r#"[{"name":"set_led","description":"Control the onboard RGB LED","#,
        r#""parameters":{"type":"object","properties":{"#,
        r#""color":{"type":"string","enum":["red","green","blue","yellow","purple","white"]},"#,
        r#""mode":{"type":"string","enum":["solid","flash","off"]},"#,
        r#""duration_seconds":{"type":"number","minimum":0.1,"maximum":60}},"#,
        r#""required":["color","mode"]}}]"#
    );

    /// Whether the whole string is accepted and the call completes.
    fn feed(g: &Grammar, s: &str) -> bool {
        let mut st = GState::new(g);
        st.open();
        for &c in s.as_bytes() {
            if !st.byte(c) {
                return false;
            }
        }
        st.complete()
    }

    #[test]
    fn compiles_the_reference_schema() {
        let g = compile(SCHEMA).unwrap();
        assert_eq!(g.tools.len(), 1);
        assert_eq!(g.tools[0].props.len(), 3);
        assert_eq!(g.tools[0].required, 0x3);
        assert_eq!(g.tools[0].props[0].enums.len(), 6);
        assert_eq!(g.tools[0].props[2].min, Some(0.1));
        assert_eq!(g.tools[0].props[2].max, Some(60.0));
    }

    #[test]
    fn accepts_valid_calls() {
        let g = compile(SCHEMA).unwrap();
        let ok = [
            ("empty call (refusal)", r#"[]"#),
            (
                "both required props",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash"}}]"#,
            ),
            (
                "with optional number",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash","duration_seconds":2}}]"#,
            ),
            (
                "props in any order",
                r#"[{"name":"set_led","arguments":{"mode":"off","color":"white"}}]"#,
            ),
            (
                "fractional number",
                r#"[{"name":"set_led","arguments":{"color":"blue","mode":"solid","duration_seconds":0.5}}]"#,
            ),
            (
                "two calls",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash"}},{"name":"set_led","arguments":{"color":"blue","mode":"off"}}]"#,
            ),
        ];
        for (why, s) in ok {
            assert!(feed(&g, s), "{why} should be accepted: {s}");
        }
    }

    #[test]
    fn rejects_invalid_calls() {
        let g = compile(SCHEMA).unwrap();
        let bad = [
            (
                "unknown tool name",
                r#"[{"name":"set_lights","arguments":{}}]"#,
            ),
            (
                "hallucinated tool",
                r#"[{"name":"set_onboard_lighting","arguments":{}}]"#,
            ),
            (
                "colour outside enum",
                r#"[{"name":"set_led","arguments":{"color":"cyan","mode":"flash"}}]"#,
            ),
            (
                "mode outside enum",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"blink"}}]"#,
            ),
            (
                "missing required 'mode'",
                r#"[{"name":"set_led","arguments":{"color":"red"}}]"#,
            ),
            (
                "missing both required",
                r#"[{"name":"set_led","arguments":{}}]"#,
            ),
            (
                "number above maximum",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash","duration_seconds":99}}]"#,
            ),
            (
                "undeclared property",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash","brightness":5}}]"#,
            ),
            (
                "duplicate property",
                r#"[{"name":"set_led","arguments":{"color":"red","color":"blue","mode":"off"}}]"#,
            ),
            (
                "trailing comma",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash",}}]"#,
            ),
            (
                "unterminated array",
                r#"[{"name":"set_led","arguments":{"color":"red","mode":"flash"}}"#,
            ),
            (
                "enum prefix only",
                r#"[{"name":"set_led","arguments":{"mode":"flash","color":"gre"}}]"#,
            ),
        ];
        for (why, s) in bad {
            assert!(!feed(&g, s), "{why} should be rejected: {s}");
        }
    }

    #[test]
    fn rejects_constructs_outside_the_supported_scope() {
        let bad = [
            (
                "nested object",
                r#"[{"name":"a","parameters":{"properties":{"x":{"type":"object"}}}}]"#,
            ),
            (
                "array type",
                r#"[{"name":"a","parameters":{"properties":{"x":{"type":"array"}}}}]"#,
            ),
            (
                "null type",
                r#"[{"name":"a","parameters":{"properties":{"x":{"type":"null"}}}}]"#,
            ),
            ("no name", r#"[{"description":"x"}]"#),
            ("not an array", r#"{"name":"a"}"#),
        ];
        for (why, s) in bad {
            assert!(compile(s).is_err(), "{why} should fail to compile");
        }
        assert!(compile("[]").is_ok(), "an empty tool array should compile");
    }

    #[test]
    fn compacts_json_outside_string_literals() {
        assert_eq!(
            json_compact("[{\n  \"name\" : \"a b\",\n  \"x\": [1, 2]\n}]"),
            r#"[{"name":"a b","x":[1,2]}]"#
        );
        // Whitespace inside string literals must survive untouched.
        assert_eq!(
            json_compact(r#"{"a":"  keep  me  "}"#),
            r#"{"a":"  keep  me  "}"#
        );
        // An escaped quote must not end the string early.
        assert_eq!(
            json_compact(r#"{"a":"x\" y","b": 1}"#),
            r#"{"a":"x\" y","b":1}"#
        );
    }

    #[test]
    fn an_enum_at_the_limit_does_not_overflow_the_mask() {
        assert_eq!(mask(0), 0);
        assert_eq!(mask(1), 1);
        assert_eq!(mask(12), 0x0FFF);
        assert_eq!(mask(GR_MAX_ENUM), 0xFFFF);
    }

    #[test]
    fn a_free_form_string_takes_any_printable_byte() {
        let g = compile(r#"[{"name":"qr","parameters":{"properties":{"text":{"type":"string"}},"required":["text"]}}]"#).unwrap();
        assert!(feed(
            &g,
            r#"[{"name":"qr","arguments":{"text":"https://a.b/c?d=e&f"}}]"#
        ));
        // A raw control byte is not legal inside the value.
        assert!(!feed(
            &g,
            "[{\"name\":\"qr\",\"arguments\":{\"text\":\"a\nb\"}}]"
        ));
    }

    #[test]
    fn an_integer_property_rejects_a_fractional_value() {
        let g = compile(
            r#"[{"name":"serve","parameters":{"properties":{"port":{"type":"integer","minimum":1,"maximum":65535}}}}]"#,
        )
        .unwrap();
        assert!(feed(&g, r#"[{"name":"serve","arguments":{"port":8080}}]"#));
        assert!(!feed(&g, r#"[{"name":"serve","arguments":{"port":80.5}}]"#));
        assert!(!feed(
            &g,
            r#"[{"name":"serve","arguments":{"port":99999}}]"#
        ));
    }
}
