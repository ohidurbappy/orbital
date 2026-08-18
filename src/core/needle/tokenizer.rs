//! SentencePiece BPE over the RAW tokenizer tensor.
//!
//! Everything here is byte-level: piece surfaces are raw bytes, and the encoder
//! works over an escaped byte buffer, so a multi-byte character that BPE splits
//! mid-sequence still round-trips through the byte-fallback pieces.

use std::collections::HashMap;

/// U+2581 LOWER ONE EIGHTH BLOCK, SentencePiece's space marker.
const SP_SPACE: &[u8] = &[0xE2, 0x96, 0x81];

/// Piece types.
pub const TK_UNKNOWN: u8 = 1;
pub const TK_CONTROL: u8 = 2;
pub const TK_USER_DEFINED: u8 = 3;
pub const TK_BYTE: u8 = 4;

/// Special ids, fixed by the training recipe (`needle/model/tokenizer.py`).
pub const EOS_ID: u32 = 1;
pub const BOS_ID: u32 = 2;
pub const IM_END_ID: u32 = 5;
pub const THINK_START_ID: u32 = 6;
pub const THINK_END_ID: u32 = 7;
pub const TOOL_CALL_START_ID: u32 = 10;
pub const TOOL_CALL_END_ID: u32 = 11;

struct Piece {
    surface: Vec<u8>,
    typ: u8,
    score: f32,
}

/// A SentencePiece BPE reader over the RAW tokenizer blob.
pub struct Tokenizer {
    pub unk_id: u32,
    pub add_dummy_prefix: bool,
    pub byte_fallback: bool,

    pieces: Vec<Piece>,
    lookup: HashMap<Vec<u8>, u32>,
    byte_id: [i32; 256],
    /// USER_DEFINED ids, longest surface first.
    markers: Vec<u32>,
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Map `"<0xAB>"` to `0xAB`, or `None`.
fn byte_piece_value(s: &[u8]) -> Option<u8> {
    if s.len() != 6 || s[0] != b'<' || s[1] != b'0' || s[2] != b'x' || s[5] != b'>' {
        return None;
    }
    let hi = (s[3] as char).to_digit(16)?;
    let lo = (s[4] as char).to_digit(16)?;
    Some((hi << 4 | lo) as u8)
}

/// Length in bytes of the UTF-8 sequence starting with `c`. A stray
/// continuation byte is treated as its own symbol.
fn utf8_len(c: u8) -> usize {
    match c {
        c if c < 0x80 => 1,
        c if c & 0xE0 == 0xC0 => 2,
        c if c & 0xF0 == 0xE0 => 3,
        c if c & 0xF8 == 0xF0 => 4,
        _ => 1,
    }
}

/// One BPE symbol: a span of the escaped buffer, in a doubly-linked list so
/// merges are O(1) once the best pair is known.
#[derive(Clone, Copy)]
struct Sym {
    off: usize,
    length: usize,
    next: i32,
}

impl Tokenizer {
    /// Build the lookup tables over a RAW tokenizer blob.
    pub fn new(blob: &[u8]) -> Result<Self, &'static str> {
        if blob.len() < 24 {
            return Err("tokenizer: blob too short");
        }
        let n_pieces = u32_at(blob, 0);
        if n_pieces == 0 || n_pieces > 65534 {
            return Err("tokenizer: bad piece count");
        }

        let mut pieces = Vec::with_capacity(n_pieces as usize);
        let mut off = 24;
        for _ in 0..n_pieces {
            if off + 7 > blob.len() {
                return Err("tokenizer: truncated record");
            }
            let score = f32::from_bits(u32_at(blob, off));
            let typ = blob[off + 4];
            let slen = u16::from_le_bytes([blob[off + 5], blob[off + 6]]) as usize;
            off += 7;
            if off + slen > blob.len() {
                return Err("tokenizer: truncated surface");
            }
            pieces.push(Piece {
                surface: blob[off..off + slen].to_vec(),
                typ,
                score,
            });
            off += slen;
        }

        let mut lookup: HashMap<Vec<u8>, u32> = HashMap::with_capacity(n_pieces as usize * 2);
        let mut byte_id = [-1i32; 256];
        let mut markers = Vec::new();
        for (i, p) in pieces.iter().enumerate() {
            // Later duplicates must not shadow the canonical (lower) id.
            lookup.entry(p.surface.clone()).or_insert(i as u32);
            match p.typ {
                TK_BYTE => {
                    if let Some(v) = byte_piece_value(&p.surface) {
                        byte_id[v as usize] = i as i32;
                    }
                }
                TK_USER_DEFINED => markers.push(i as u32),
                _ => {}
            }
        }
        // Longest surface first, so a marker is never split by a shorter
        // prefix. Rust's sort is stable, matching sort.SliceStable.
        markers.sort_by_key(|&i| std::cmp::Reverse(pieces[i as usize].surface.len()));

        Ok(Tokenizer {
            unk_id: u32_at(blob, 16),
            add_dummy_prefix: blob[20] != 0,
            byte_fallback: blob[21] != 0,
            pieces,
            lookup,
            byte_id,
            markers,
        })
    }

    /// The surface bytes of one piece, empty for an out-of-range id.
    pub fn piece(&self, id: u32) -> &[u8] {
        match self.pieces.get(id as usize) {
            Some(p) => &p.surface,
            None => &[],
        }
    }

    /// Merge one segment of the escaped buffer and append its ids.
    fn bpe_segment(&self, buf: &[u8], off: usize, length: usize, ids: &mut Vec<u32>) {
        if length == 0 {
            return;
        }
        let mut syms: Vec<Sym> = Vec::with_capacity(length);
        let mut i = 0;
        while i < length {
            let mut cl = utf8_len(buf[off + i]);
            if i + cl > length {
                cl = length - i;
            }
            let n = syms.len() as i32;
            syms.push(Sym {
                off: off + i,
                length: cl,
                next: n + 1,
            });
            i += cl;
        }
        let last = syms.len() - 1;
        syms[last].next = -1;

        // Repeatedly merge the highest-scoring adjacent pair present in the
        // vocabulary, exactly as RefTokenizer._bpe does.
        loop {
            let mut best: i32 = -1;
            let mut best_score = 0f32;
            let mut a: i32 = 0;
            while a >= 0 && syms[a as usize].next >= 0 {
                let s = syms[a as usize];
                let b = syms[s.next as usize];
                let total = s.length + b.length;
                if let Some(&id) = self.lookup.get(&buf[s.off..s.off + total]) {
                    let score = self.pieces[id as usize].score;
                    if best < 0 || score > best_score {
                        best = a;
                        best_score = score;
                    }
                }
                a = s.next;
            }
            if best < 0 {
                break;
            }
            let bi = syms[best as usize].next as usize;
            syms[best as usize].length += syms[bi].length;
            syms[best as usize].next = syms[bi].next;
        }

        let mut i: i32 = 0;
        loop {
            let s = syms[i as usize];
            if let Some(&id) = self.lookup.get(&buf[s.off..s.off + s.length]) {
                ids.push(id);
            } else if self.byte_fallback {
                for k in 0..s.length {
                    let bid = self.byte_id[buf[s.off + k] as usize];
                    ids.push(if bid >= 0 { bid as u32 } else { self.unk_id });
                }
            } else {
                ids.push(self.unk_id);
            }
            if s.next < 0 {
                break;
            }
            i = s.next;
        }
    }

    /// Tokenize text using the tokenizer's own dummy-prefix setting.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        self.encode_ex(text, self.add_dummy_prefix)
    }

    /// Tokenize text, overriding the dummy-prefix flag.
    ///
    /// Pass `false` when encoding a continuation: the cached prefix already
    /// consumed the dummy space, and re-adding it shifts every following token.
    pub fn encode_ex(&self, text: &str, add_dummy: bool) -> Vec<u32> {
        if text.is_empty() {
            return Vec::new();
        }
        // Escape: every space becomes the 3-byte marker, plus the dummy prefix.
        let mut esc = Vec::with_capacity(text.len() * 3 + 3);
        if add_dummy {
            esc.extend_from_slice(SP_SPACE);
        }
        for &b in text.as_bytes() {
            if b == b' ' {
                esc.extend_from_slice(SP_SPACE);
            } else {
                esc.push(b);
            }
        }

        let mut ids = Vec::new();
        let (mut seg_start, mut i) = (0usize, 0usize);
        while i < esc.len() {
            let mut hit = false;
            for &id in &self.markers {
                let ms = &self.pieces[id as usize].surface;
                if ms.is_empty() || i + ms.len() > esc.len() || esc[i..i + ms.len()] != ms[..] {
                    continue;
                }
                self.bpe_segment(&esc, seg_start, i - seg_start, &mut ids);
                ids.push(id);
                i += ms.len();
                seg_start = i;
                hit = true;
                break;
            }
            if !hit {
                i += utf8_len(esc[i]);
            }
        }
        self.bpe_segment(&esc, seg_start, esc.len() - seg_start, &mut ids);
        ids
    }

    /// Turn ids back into UTF-8.
    ///
    /// `strip_dummy` is true for a whole sequence and false when streaming
    /// token by token: the dummy-prefix rule applies once per sequence, and
    /// applying it per token eats the real spaces between words.
    pub fn decode_ex(&self, ids: &[u32], strip_dummy: bool) -> String {
        let mut out: Vec<u8> = Vec::with_capacity(ids.len() * 4);
        for &id in ids {
            let Some(p) = self.pieces.get(id as usize) else {
                continue;
            };
            if p.typ == TK_BYTE {
                if let Some(v) = byte_piece_value(&p.surface) {
                    out.push(v);
                }
                continue;
            }
            if p.typ == TK_CONTROL || p.typ == TK_UNKNOWN {
                continue;
            }
            out.extend_from_slice(&p.surface);
        }

        // Unescape U+2581 back to a space, in place.
        let mut w = 0;
        let mut r = 0;
        while r < out.len() {
            if out[r..].starts_with(SP_SPACE) {
                out[w] = b' ';
                w += 1;
                r += 3;
            } else {
                out[w] = out[r];
                w += 1;
                r += 1;
            }
        }
        out.truncate(w);

        // Drop the dummy prefix the encoder added.
        let body = if strip_dummy && self.add_dummy_prefix && out.first() == Some(&b' ') {
            &out[1..]
        } else {
            &out[..]
        };
        // Byte-fallback pieces can split a character across tokens, so a
        // single-token decode is legitimately partial UTF-8 mid-sequence.
        String::from_utf8_lossy(body).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_byte_piece_name() {
        assert_eq!(byte_piece_value(b"<0xAB>"), Some(0xAB));
        assert_eq!(byte_piece_value(b"<0x0f>"), Some(0x0F));
        assert_eq!(byte_piece_value(b"<0xZZ>"), None);
        assert_eq!(byte_piece_value(b"hello"), None);
        assert_eq!(byte_piece_value(b"<0xAB"), None);
    }

    #[test]
    fn measures_utf8_sequence_lengths() {
        assert_eq!(utf8_len(b'a'), 1);
        assert_eq!(utf8_len(0xC3), 2);
        assert_eq!(utf8_len(0xE2), 3);
        assert_eq!(utf8_len(0xF0), 4);
        // A stray continuation byte stands alone rather than over-reading.
        assert_eq!(utf8_len(0x96), 1);
    }

    #[test]
    fn a_short_blob_is_rejected() {
        assert!(Tokenizer::new(&[0u8; 8]).is_err());
    }

    #[test]
    fn a_zero_piece_count_is_rejected() {
        assert!(Tokenizer::new(&[0u8; 32]).is_err());
    }
}
