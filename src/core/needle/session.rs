//! One model driven against a fixed tool schema.
//!
//! The constant prompt prefix — the rendered `<tools>` block — is prefilled
//! once and snapshotted, so a request costs only its query tokens instead of
//! the whole schema.

use super::grammar::{compile, json_compact, Grammar};
use super::model::{Model, State};
use super::parse::{parse_calls, Call};
use super::sample::Sampler;
use super::tokenizer::{
    BOS_ID, EOS_ID, IM_END_ID, THINK_END_ID, THINK_START_ID, TOOL_CALL_START_ID,
};

pub struct Session<'a> {
    model: Model<'a>,
    state: State,
    grammar: Grammar,
    /// The schema, compacted.
    pub tools_json: String,
    pub prefix_len: usize,
    primed: bool,
}

/// One completed generation.
#[derive(Debug, Clone)]
pub struct Generated {
    /// The raw generated text, including the `<tool_call>` block.
    pub text: String,
    /// Parsed tool calls; empty for the empty call `[]`.
    pub calls: Vec<Call>,
    /// Calibrated groundedness in `[0,1]`, or -1 with no confidence head.
    pub confidence: f32,
    pub tokens: usize,
    /// Hit `max_new` before finishing.
    pub truncated: bool,
}

impl<'a> Session<'a> {
    /// Compile the tool schema and open a session over a model.
    ///
    /// The schema is compacted before it reaches the model: Needle was trained
    /// on whitespace-free schemas and an indented one degrades it *silently* —
    /// it begins citing tools that were never declared, with no error anywhere.
    /// Callers may indent their schema freely.
    pub fn new(model: Model<'a>, tools_json: &str) -> Result<Self, &'static str> {
        let packed = json_compact(tools_json);
        let grammar = compile(&packed)?;
        let state = model.new_state();
        Ok(Session {
            model,
            state,
            grammar,
            tools_json: packed,
            prefix_len: 0,
            primed: false,
        })
    }

    /// The token ids of the cacheable prompt prefix.
    fn prefix_tokens(&self) -> Vec<u32> {
        let pre = format!("<|im_start|>user\n<tools>{}</tools>", self.tools_json);
        let mut ids = vec![BOS_ID];
        ids.extend(self.model.tok.encode(&pre));
        ids
    }

    /// Prefill the constant prefix and snapshot it. `progress` is called with
    /// `(done, total)` after each token.
    pub fn prime(&mut self, mut progress: Option<&mut dyn FnMut(usize, usize)>) {
        let ids = self.prefix_tokens();
        self.model.reset(&mut self.state);
        for (i, &id) in ids.iter().enumerate() {
            self.model.step_hidden(&mut self.state, id);
            if let Some(p) = progress.as_deref_mut() {
                p(i + 1, ids.len());
            }
        }
        self.model.snapshot(&mut self.state);
        self.prefix_len = ids.len();
        self.primed = true;
    }

    /// Run one request against the primed prefix. `on_token` receives each
    /// decoded token's text as it is produced.
    ///
    /// The query suffix is encoded with the dummy prefix disabled: the cached
    /// prefix already consumed it, and splitting at the `</tools>` marker
    /// guarantees no BPE merge spans the boundary, so prefix+suffix encodes
    /// identically to the whole string.
    pub fn generate(
        &mut self,
        query: &str,
        max_new: usize,
        no_think: bool,
        mut on_token: Option<&mut dyn FnMut(&str)>,
    ) -> Result<Generated, &'static str> {
        if !self.primed {
            self.prime(None);
        }
        self.model.rewind(&mut self.state);
        let mut smp = Sampler::new(&self.model.tok, &self.grammar);

        let suf = format!("\n{query}<|im_end|>\n<|im_start|>assistant\n");
        for id in self.model.tok.encode_ex(&suf, false) {
            self.model.step_hidden(&mut self.state, id);
        }

        let mut text = String::new();
        let mut tokens = 0usize;
        let mut truncated = true;

        let emit = |smp: &mut Sampler,
                    model: &Model,
                    state: &mut State,
                    text: &mut String,
                    on_token: &mut Option<&mut dyn FnMut(&str)>,
                    id: u32| {
            smp.accept(id);
            let piece = model.tok.decode_ex(&[id], false);
            text.push_str(&piece);
            if let Some(f) = on_token.as_deref_mut() {
                f(&piece);
            }
            model.step_hidden(state, id);
        };

        // Optionally skip the reasoning block: force an empty <think></think>
        // and open the call directly. Verified in the reference engine to
        // produce identical calls at roughly half the generated tokens.
        if no_think {
            let mut forced = vec![THINK_START_ID, THINK_END_ID];
            forced.extend(self.model.tok.encode_ex("\n", false));
            forced.push(TOOL_CALL_START_ID);
            for id in forced {
                emit(
                    &mut smp,
                    &self.model,
                    &mut self.state,
                    &mut text,
                    &mut on_token,
                    id,
                );
            }
        }

        for _ in 0..max_new {
            let Some(id) = smp.sample_hidden(&self.model, &mut self.state) else {
                return Err("no legal token");
            };
            if id == EOS_ID || id == IM_END_ID {
                truncated = false;
                break;
            }
            emit(
                &mut smp,
                &self.model,
                &mut self.state,
                &mut text,
                &mut on_token,
                id,
            );
            tokens += 1;
            if smp.finished {
                truncated = false;
                break;
            }
        }

        Ok(Generated {
            calls: parse_calls(&text),
            confidence: self.model.confidence(&self.state),
            text,
            tokens,
            truncated,
        })
    }
}
