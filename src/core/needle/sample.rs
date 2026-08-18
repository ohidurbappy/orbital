//! Grammar-constrained greedy sampling.
//!
//! The grammar is disengaged while the model writes its `<think>` block and
//! engages on `<tool_call>`, matching how the model was trained: only the call
//! is constrained, the reasoning stays legible.

use super::grammar::{GState, Grammar};
use super::model::{Model, State};
use super::tokenizer::{Tokenizer, TOOL_CALL_END_ID, TOOL_CALL_START_ID};

/// The most candidates the constrained fast path scores directly. Beyond this
/// it is cheaper to project the whole vocabulary.
const MAX_CAND: usize = 512;

/// Picks the highest-logit token whose bytes the grammar can accept. Candidates
/// are trialled on a copy of the state, so a rejected token costs nothing.
pub struct Sampler<'a> {
    tok: &'a Tokenizer,
    st: GState<'a>,
    /// Inside `<tool_call>`.
    pub engaged: bool,
    /// `</tool_call>` emitted.
    pub finished: bool,
    cand: Vec<u32>,
    score: Vec<f32>,
}

impl<'a> Sampler<'a> {
    pub fn new(tok: &'a Tokenizer, g: &'a Grammar) -> Self {
        Sampler {
            tok,
            st: GState::new(g),
            engaged: false,
            finished: false,
            cand: Vec::with_capacity(MAX_CAND),
            score: vec![0.0; MAX_CAND],
        }
    }

    /// Whether the grammar accepts every byte of a token's surface. On success
    /// `out` receives the advanced state.
    fn token_ok(&self, id: u32, out: Option<&mut GState<'a>>) -> bool {
        let mut trial = self.st;
        let surf = self.tok.piece(id);
        if surf.is_empty() {
            return false;
        }
        for &b in surf {
            // SentencePiece's space marker is U+2581; inside a JSON call the
            // model emits real bytes, so a marker byte here is not legal.
            if !trial.byte(b) {
                return false;
            }
        }
        if let Some(out) = out {
            *out = trial;
        }
        true
    }

    /// Choose the next token from full logits, honouring the grammar.
    fn sample(&self, logits: &[f32]) -> Option<u32> {
        if !self.engaged {
            // Unconstrained: plain argmax over the whole vocabulary.
            return logits
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i as u32);
        }
        // Once the call is complete only </tool_call> may follow.
        if self.st.complete() {
            return Some(TOOL_CALL_END_ID);
        }
        let mut best = None;
        let mut bv = -1e30f32;
        // Control pieces carry no bytes and would slip through the byte check,
        // so ids below 16 are excluded explicitly while constrained.
        for (j, &v) in logits.iter().enumerate().skip(16) {
            if v <= bv || !self.token_ok(j as u32, None) {
                continue;
            }
            bv = v;
            best = Some(j as u32);
        }
        best
    }

    /// Pick the next token from `st.y`, scoring only the tokens the grammar
    /// currently permits.
    ///
    /// Inside a tool call that is typically a few dozen of 8192, so the
    /// vocabulary projection — otherwise the second-largest cost per token —
    /// nearly vanishes. Falls back to full logits when unconstrained, or when
    /// too many tokens are legal.
    pub fn sample_hidden(&mut self, m: &Model, st: &mut State) -> Option<u32> {
        if !self.engaged {
            m.logits_all(st);
            return self.sample(&st.logits);
        }
        if self.st.complete() {
            return Some(TOOL_CALL_END_ID);
        }

        // Enumerate what the grammar allows. Byte-checking the vocabulary costs
        // ~25K byte steps, far less than 4.2M multiply-adds.
        self.cand.clear();
        let mut overflow = false;
        for j in 16..m.vocab {
            if !self.token_ok(j, None) {
                continue;
            }
            if self.cand.len() >= MAX_CAND {
                overflow = true;
                break;
            }
            self.cand.push(j);
        }
        if overflow {
            // Too many legal tokens: the full projection is cheaper.
            m.logits_all(st);
            return self.sample(&st.logits);
        }
        if self.cand.is_empty() {
            return None;
        }

        let score = &mut self.score[..self.cand.len()];
        m.logits_subset(st, &self.cand, score);
        let mut best = None;
        let mut bv = -1e30f32;
        for (j, &v) in score.iter().enumerate() {
            if v > bv {
                bv = v;
                best = Some(self.cand[j]);
            }
        }
        best
    }

    /// Advance sampler state by a chosen token. Call for every token fed back
    /// into the model.
    pub fn accept(&mut self, id: u32) {
        match id {
            TOOL_CALL_START_ID => {
                self.engaged = true;
                self.st.open();
                return;
            }
            TOOL_CALL_END_ID => {
                self.engaged = false;
                self.finished = true;
                return;
            }
            _ => {}
        }
        if self.engaged {
            let mut trial = self.st;
            if self.token_ok(id, Some(&mut trial)) {
                self.st = trial;
            }
        }
    }
}
