//! Reader for the Cactus `.cact` v3 deployment blob.
//!
//! Ported from the Go engine in `needle-2-test/needle/cact.go`, which is itself
//! a port of the portable C99 reference. The blob is read **in place**: nothing
//! is copied, so `Cact` only ever borrows the bytes it was opened over.

/// Magic word at the head of every v3 blob.
const CACT_TAG: u32 = 0x05E1_2A83;
/// Bytes per directory record.
const CACT_REC_SIZE: usize = 44;
/// Header length, in 32-bit words.
const HDR_WORDS: usize = 30;
const HDR_BYTES: usize = HDR_WORDS * 4;

/// `dtype` codes in a directory record.
pub const DT_CQ: u8 = 3; // Cactus-Quants; `bits` gives the width (2, 3 or 4)
pub const DT_RAW: u8 = 4;

/// The v3 geometry, in on-disk field order.
///
/// Every header word is decoded and kept even where this engine has no use for
/// it, so the struct doubles as the format's documentation and a future field
/// does not have to re-derive the offsets.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct Header {
    pub tag: u32,
    pub num_tensors: u32,
    pub codebook_len: u32,
    pub kv_window: u32,
    pub kv_bits: u32,
    pub vocab_size: u32,
    pub d_model: u32,
    pub num_heads: u32,
    pub num_kv_heads: u32,
    pub num_layers: u32,
    pub head_dim: u32,
    pub max_seq_len: u32,
    pub attn_dim: u32,
    pub mhc_lanes: u32,
    pub engram_slots: u32,
    pub engram_sub_dim: u32,
    pub engram_conv_taps: u32,
    pub engram_tables: u32,
    pub engram_dilation: u32,
    pub num_orders: u32,
    pub orders: [u32; 4],
    pub num_sites: u32,
    pub sites: [u32; 4],
    pub rope_theta: f32,
}

/// One decoded directory record.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tensor {
    pub dtype: u8,
    pub ndim: u8,
    pub shape: [u32; 4],
    pub offset: u64,
    pub nbytes: u64,
    pub group: u32,
    pub bits: u32,
}

impl Tensor {
    /// The padded reduction length: `shape[1]` rounded up to `group`.
    pub fn in_pad(&self) -> u32 {
        if self.group == 0 {
            return 0;
        }
        self.shape[1].div_ceil(self.group) * self.group
    }

    /// Bytes of packed indices per output row.
    pub fn row_bytes(&self) -> u32 {
        self.in_pad() * self.bits / 8
    }

    /// Quantisation groups per output row.
    pub fn groups(&self) -> u32 {
        if self.group == 0 {
            return 0;
        }
        self.in_pad() / self.group
    }
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([
        b[i],
        b[i + 1],
        b[i + 2],
        b[i + 3],
        b[i + 4],
        b[i + 5],
        b[i + 6],
        b[i + 7],
    ])
}

/// A parsed blob, borrowed in place.
pub struct Cact<'a> {
    pub base: &'a [u8],
    pub h: Header,
    /// `cb2 | cb3 | cb4`, already scaled by `1/sqrt(group)` on disk.
    pub codebook: Vec<f32>,
    dir: &'a [u8],
    pub n: u32,
}

impl<'a> Cact<'a> {
    /// The codebook slice for a width; `bits` must be 2, 3 or 4.
    pub fn codebook_for(&self, bits: u32) -> &[f32] {
        match bits {
            2 => &self.codebook[0..4],
            3 => &self.codebook[4..12],
            4 => &self.codebook[12..28],
            _ => &[],
        }
    }

    /// Parse a blob in place.
    pub fn open(blob: &'a [u8]) -> Result<Self, &'static str> {
        if blob.len() < HDR_BYTES {
            return Err("cact: blob shorter than header");
        }
        let mut w = [0u32; HDR_WORDS];
        for (i, word) in w.iter_mut().enumerate() {
            *word = u32_at(blob, i * 4);
        }
        let h = Header {
            tag: w[0],
            num_tensors: w[1],
            codebook_len: w[2],
            kv_window: w[3],
            kv_bits: w[4],
            vocab_size: w[5],
            d_model: w[6],
            num_heads: w[7],
            num_kv_heads: w[8],
            num_layers: w[9],
            head_dim: w[10],
            max_seq_len: w[11],
            attn_dim: w[12],
            mhc_lanes: w[13],
            engram_slots: w[14],
            engram_sub_dim: w[15],
            engram_conv_taps: w[16],
            engram_tables: w[17],
            engram_dilation: w[18],
            num_orders: w[19],
            orders: [w[20], w[21], w[22], w[23]],
            num_sites: w[24],
            sites: [w[25], w[26], w[27], w[28]],
            rope_theta: f32::from_bits(w[29]),
        };

        if h.tag != CACT_TAG {
            return Err("cact: bad magic tag");
        }
        // Only the geometry this engine actually implements.
        if h.d_model == 0
            || h.num_layers == 0
            || h.num_heads == 0
            || h.num_kv_heads == 0
            || h.head_dim == 0
            || h.num_heads % h.num_kv_heads != 0
            || h.num_heads * h.head_dim != h.attn_dim
            || h.num_orders > 4
            || h.num_sites > 4
        {
            return Err("cact: unsupported geometry");
        }

        let cb_off = HDR_BYTES as u64;
        let cb_len = h.codebook_len as u64 * 4;
        let dir_off = cb_off + cb_len;
        let dir_len = h.num_tensors as u64 * CACT_REC_SIZE as u64;
        if dir_off + dir_len > blob.len() as u64 {
            return Err("cact: directory runs past blob");
        }
        if h.codebook_len < 28 {
            return Err("cact: codebook too short");
        }

        let mut codebook = vec![0f32; h.codebook_len as usize];
        for (i, slot) in codebook.iter_mut().enumerate() {
            *slot = f32::from_bits(u32_at(blob, cb_off as usize + i * 4));
        }

        Ok(Cact {
            base: blob,
            n: h.num_tensors,
            h,
            codebook,
            dir: &blob[dir_off as usize..(dir_off + dir_len) as usize],
        })
    }

    /// Decode directory record `i`.
    pub fn tensor_at(&self, i: u32) -> Tensor {
        let mut t = Tensor::default();
        if i >= self.n {
            return t;
        }
        let r = &self.dir[i as usize * CACT_REC_SIZE..];
        t.dtype = r[0];
        t.ndim = r[1];
        // r[2..4] is padding.
        t.shape = [u32_at(r, 4), u32_at(r, 8), u32_at(r, 12), u32_at(r, 16)];
        t.offset = u64_at(r, 20);
        t.nbytes = u64_at(r, 28);
        t.group = u32_at(r, 36);
        t.bits = u32_at(r, 40);
        // Shape words past ndim are written as zero; normalise them.
        for d in t.ndim as usize..4 {
            t.shape[d] = 0;
        }
        t
    }

    /// Tensor `t`'s payload, or `None` if the record runs past the blob.
    pub fn data(&self, t: &Tensor) -> Option<&'a [u8]> {
        let len = self.base.len() as u64;
        if t.offset > len || t.nbytes > len - t.offset {
            return None;
        }
        Some(&self.base[t.offset as usize..(t.offset + t.nbytes) as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_pad_rounds_up_to_the_group() {
        let t = Tensor {
            shape: [4, 100, 0, 0],
            group: 32,
            bits: 2,
            ..Tensor::default()
        };
        assert_eq!(t.in_pad(), 128);
        assert_eq!(t.groups(), 4);
        assert_eq!(t.row_bytes(), 32); // 128 weights * 2 bits / 8
    }

    #[test]
    fn an_exact_multiple_is_left_alone() {
        let t = Tensor {
            shape: [1, 512, 0, 0],
            group: 64,
            bits: 4,
            ..Tensor::default()
        };
        assert_eq!(t.in_pad(), 512);
        assert_eq!(t.groups(), 8);
        assert_eq!(t.row_bytes(), 256);
    }

    #[test]
    fn a_short_blob_is_rejected() {
        assert!(Cact::open(&[0u8; 8]).is_err());
    }

    #[test]
    fn a_bad_magic_tag_is_rejected() {
        let blob = [0u8; HDR_BYTES + 64];
        assert_eq!(Cact::open(&blob).err(), Some("cact: bad magic tag"));
    }

    #[test]
    fn a_zero_group_does_not_divide_by_zero() {
        let t = Tensor::default();
        assert_eq!(t.in_pad(), 0);
        assert_eq!(t.groups(), 0);
    }
}
