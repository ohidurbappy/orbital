//! Cactus-Quants kernels: the pair-LUT GEMV and the Walsh-Hadamard transform.
//!
//! Weights are **never dequantised**. A CQ group stores one codebook index per
//! weight plus an FP16 group norm, and the logical weight is
//! `(codebook[idx] * norm) @ H` for a Walsh-Hadamard `H`. Since `H` is
//! symmetric and orthonormal, `<w, x> == <cb[idx] * norm, H @ x>` — so the
//! *activation* is transformed once per group and each output row costs one
//! lookup and one multiply-add per weight.

use super::cact::{Cact, Tensor, DT_CQ};

/// Convert an IEEE half to `f32`.
///
/// Only ever called at load time: group norms and FP16 tensors are expanded to
/// `f32` once (see [`QT::new`] and [`f16_slice`]), so the hot loops never touch
/// a half. That is worth ~1.4 MB of memory and cut priming time measurably in
/// the reference engine.
pub fn f16(h: u16) -> f32 {
    let sign = (h as u32 & 0x8000) << 16;
    let exp = (h >> 10) as u32 & 0x1F;
    let man = h as u32 & 0x3FF;

    let bits = if exp == 0 {
        if man == 0 {
            sign // +-0
        } else {
            // Subnormal: renormalise into a float32 exponent.
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 0x1F {
        sign | 0x7F80_0000 | (man << 13) // inf / nan
    } else {
        sign | ((exp + 127 - 15) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

fn f16_at(b: &[u8], i: usize) -> f32 {
    f16(u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]))
}

/// The in-place unnormalised fast Walsh-Hadamard transform. `n` must be a power
/// of two; apply `1/sqrt(n)` yourself for the orthonormal `H`.
pub fn fwht(x: &mut [f32], n: usize) {
    let mut length = 1;
    while length < n {
        let mut i = 0;
        while i < n {
            for j in i..i + length {
                let (a, b) = (x[j], x[j + length]);
                x[j] = a + b;
                x[j + length] = a - b;
            }
            i += length << 1;
        }
        length <<= 1;
    }
}

/// A CQ tensor bound to its payload, with its group norms expanded to `f32`.
pub struct QT<'a> {
    pub t: Tensor,
    pub data: &'a [u8],
    pub norms: Vec<f32>,
}

impl<'a> QT<'a> {
    /// Bind directory entry `i` as a CQ tensor and expand its group norms.
    pub fn new(c: &Cact<'a>, i: u32) -> QT<'a> {
        let t = c.tensor_at(i);
        let data = c.data(&t).unwrap_or(&[]);
        if t.dtype != DT_CQ || data.is_empty() {
            return QT {
                t,
                data,
                norms: Vec::new(),
            };
        }
        // The norms follow the packed indices: shape[0] rows of row_bytes each.
        let n = t.shape[0] as usize * t.groups() as usize;
        let raw = &data[t.shape[0] as usize * t.row_bytes() as usize..];
        let norms = (0..n).map(|k| f16_at(raw, k)).collect();
        QT { t, data, norms }
    }
}

/// Expand an FP16 tensor to `f32` once.
///
/// Sized from the payload, not the shape: `b_res` is 3-D and its trailing
/// dimension is not carried in `shape`.
pub fn f16_slice(c: &Cact, t: &Tensor) -> Vec<f32> {
    let d = c.data(t).unwrap_or(&[]);
    (0..d.len() / 2).map(|i| f16_at(d, i)).collect()
}

/// Zero-pad `x` to `in_pad` and apply the orthonormal `H` per group, so a
/// matvec never has to materialise dequantised weights. Reusable across every
/// tensor sharing `(shape[1], group)`.
pub fn prepare(t: &Tensor, x: &[f32], xh: &mut [f32]) {
    let in_pad = t.in_pad() as usize;
    let g = t.group as usize;
    let cols = t.shape[1] as usize;
    let scale = (1.0 / (g as f64).sqrt()) as f32;

    xh[..cols].copy_from_slice(&x[..cols]);
    xh[cols..in_pad].fill(0.0);
    for blk in xh[..in_pad].chunks_mut(g) {
        fwht(blk, g);
        for v in blk.iter_mut() {
            *v *= scale;
        }
    }
}

/// Sum `cb[idx] * xh[j]` over one group. Indices are an LSB-first bitstream per
/// row: index `k` occupies bits `[k*bits, (k+1)*bits)`.
///
/// Four independent accumulators: one running sum serialises on FPU add
/// latency, which dominates this loop.
fn dot_group(p: &[u8], bit_off: u32, bits: u32, g: u32, cb: &[f32], xh: &[f32]) -> f32 {
    let g = g as usize;

    if bits == 2 && bit_off & 7 == 0 {
        let mut q = (bit_off >> 3) as usize;
        let (mut s0, mut s1, mut s2, mut s3) = (0f32, 0f32, 0f32, 0f32);
        let mut j = 0;
        while j < g {
            let (b0, b1) = (p[q], p[q + 1]);
            q += 2;
            s0 += cb[(b0 & 3) as usize] * xh[j];
            s1 += cb[(b0 >> 2 & 3) as usize] * xh[j + 1];
            s2 += cb[(b0 >> 4 & 3) as usize] * xh[j + 2];
            s3 += cb[(b0 >> 6 & 3) as usize] * xh[j + 3];
            s0 += cb[(b1 & 3) as usize] * xh[j + 4];
            s1 += cb[(b1 >> 2 & 3) as usize] * xh[j + 5];
            s2 += cb[(b1 >> 4 & 3) as usize] * xh[j + 6];
            s3 += cb[(b1 >> 6 & 3) as usize] * xh[j + 7];
            j += 8;
        }
        return (s0 + s1) + (s2 + s3);
    }

    if bits == 4 && bit_off & 7 == 0 {
        let mut q = (bit_off >> 3) as usize;
        let (mut s0, mut s1, mut s2, mut s3) = (0f32, 0f32, 0f32, 0f32);
        let mut j = 0;
        while j < g {
            let (b0, b1) = (p[q], p[q + 1]);
            q += 2;
            s0 += cb[(b0 & 15) as usize] * xh[j];
            s1 += cb[(b0 >> 4) as usize] * xh[j + 1];
            s2 += cb[(b1 & 15) as usize] * xh[j + 2];
            s3 += cb[(b1 >> 4) as usize] * xh[j + 3];
            j += 4;
        }
        return (s0 + s1) + (s2 + s3);
    }

    // Generic bit reader (covers bits == 3, and any unaligned start). Only
    // fetches the second byte when the field actually straddles, so the last
    // index of the last row never reads past the blob.
    let mut s = 0f32;
    let mask = (1u32 << bits) - 1;
    for (j, x) in xh[..g].iter().enumerate() {
        let b = bit_off as usize + j * bits as usize;
        let byt = b >> 3;
        let sh = b & 7;
        let mut w = p[byt] as u32;
        if sh + bits as usize > 8 {
            w |= (p[byt + 1] as u32) << 8;
        }
        s += cb[((w >> sh) & mask) as usize] * x;
    }
    s
}

/// `y[i] = W[r0+i] . x` over `nrows` rows, for a prepared activation. The mHC
/// phi tensors stack every layer into one tensor, so a layer's slice is a row
/// range.
pub fn gemv_rows(c: &Cact, q: &QT, xh: &[f32], r0: u32, nrows: u32, y: &mut [f32]) {
    let t = &q.t;
    let rowbytes = t.row_bytes() as usize;
    let ngroup = t.groups() as usize;
    let g = t.group as usize;
    let cb = c.codebook_for(t.bits);

    for (i, out) in y.iter_mut().take(nrows as usize).enumerate() {
        let r = r0 as usize + i;
        let row = &q.data[r * rowbytes..];
        let nrm = &q.norms[r * ngroup..];
        let mut acc = 0f32;
        for gi in 0..ngroup {
            acc += nrm[gi]
                * dot_group(
                    row,
                    (gi * g) as u32 * t.bits,
                    t.bits,
                    t.group,
                    cb,
                    &xh[gi * g..],
                );
        }
        *out = acc;
    }
}

/// `y = W @ x` for every output row, for a prepared activation.
pub fn gemv_prepared(c: &Cact, q: &QT, xh: &[f32], y: &mut [f32]) {
    gemv_rows(c, q, xh, 0, q.t.shape[0], y);
}

/// Prepare + matvec; `scratch` holds `in_pad` floats.
pub fn gemv(c: &Cact, q: &QT, x: &[f32], scratch: &mut [f32], y: &mut [f32]) {
    prepare(&q.t, x, scratch);
    gemv_prepared(c, q, scratch, y);
}

/// Floats needed for the 2-bit pair table over an activation.
pub fn lut_floats(in_pad: u32) -> usize {
    in_pad as usize / 2 * 16
}

/// Tabulate, for each adjacent PAIR of reduction positions, the 16 possible
/// partial sums. The 2-bit inner loop then becomes one indexed load and one add
/// per pair — no multiplies at all.
pub fn lut_build(cb: &[f32], xh: &[f32], in_pad: u32, lut: &mut [f32]) {
    // T[i0 | (i1 << 2)] = cb[i0]*xh[2p] + cb[i1]*xh[2p+1], matching the
    // LSB-first packing (the low 2 bits of a nibble are the earlier weight).
    for p in 0..in_pad as usize / 2 {
        let (x0, x1) = (xh[2 * p], xh[2 * p + 1]);
        let (a0, a1, a2, a3) = (cb[0] * x0, cb[1] * x0, cb[2] * x0, cb[3] * x0);
        let (b0, b1, b2, b3) = (cb[0] * x1, cb[1] * x1, cb[2] * x1, cb[3] * x1);
        let t = &mut lut[p * 16..p * 16 + 16];
        t[0] = a0 + b0;
        t[1] = a1 + b0;
        t[2] = a2 + b0;
        t[3] = a3 + b0;
        t[4] = a0 + b1;
        t[5] = a1 + b1;
        t[6] = a2 + b1;
        t[7] = a3 + b1;
        t[8] = a0 + b2;
        t[9] = a1 + b2;
        t[10] = a2 + b2;
        t[11] = a3 + b2;
        t[12] = a0 + b3;
        t[13] = a1 + b3;
        t[14] = a2 + b3;
        t[15] = a3 + b3;
    }
}

/// Sum one group through the pair table: 8 weights per iteration, four lookups.
fn dot_group_lut2(q: &[u8], t: &[f32], g: u32) -> f32 {
    let (mut s0, mut s1, mut s2, mut s3) = (0f32, 0f32, 0f32, 0f32);
    let (mut qi, mut ti) = (0usize, 0usize);
    let mut j = 0;
    while j < g {
        let (b0, b1) = (q[qi], q[qi + 1]);
        qi += 2;
        s0 += t[ti + (b0 & 15) as usize];
        s1 += t[ti + 16 + (b0 >> 4) as usize];
        s2 += t[ti + 32 + (b1 & 15) as usize];
        s3 += t[ti + 48 + (b1 >> 4) as usize];
        ti += 64; // 4 pairs consumed
        j += 8;
    }
    (s0 + s1) + (s2 + s3)
}

/// `y = W @ x` for a 2-bit tensor via the pair table.
pub fn gemv_lut2(q: &QT, lut: &[f32], y: &mut [f32]) {
    let t = &q.t;
    let g = t.group;
    let rowbytes = t.row_bytes() as usize;
    let ngroup = t.groups() as usize;
    let gbytes = g as usize / 4; // 2 bits per weight
    let gpairs = g as usize / 2;

    for (r, out) in y.iter_mut().take(t.shape[0] as usize).enumerate() {
        let row = &q.data[r * rowbytes..];
        let nrm = &q.norms[r * ngroup..];
        let mut acc = 0f32;
        for gi in 0..ngroup {
            acc += nrm[gi] * dot_group_lut2(&row[gi * gbytes..], &lut[gi * gpairs * 16..], g);
        }
        *out = acc;
    }
}

/// `y[i] = W[ids[i]] . x`, to score only the tokens a grammar currently permits
/// instead of the whole vocabulary.
pub fn gemv_gather(c: &Cact, q: &QT, xh: &[f32], ids: &[u32], y: &mut [f32]) {
    let t = &q.t;
    let g = t.group as usize;
    let ngroup = t.groups() as usize;
    let rowbytes = t.row_bytes() as usize;
    let cb = c.codebook_for(t.bits);

    for (i, &id) in ids.iter().enumerate() {
        let row = &q.data[id as usize * rowbytes..];
        let nrm = &q.norms[id as usize * ngroup..];
        let mut acc = 0f32;
        for gi in 0..ngroup {
            acc += nrm[gi]
                * dot_group(
                    row,
                    (gi * g) as u32 * t.bits,
                    t.bits,
                    t.group,
                    cb,
                    &xh[gi * g..],
                );
        }
        y[i] = acc;
    }
}

/// Reconstruct one dequantised output row into `w` (`shape[1]` floats).
///
/// Used for the embedding lookup and the engram tables, which are gathered by
/// row rather than streamed as a matvec. `scratch` holds `in_pad` floats.
pub fn dequant_row(c: &Cact, q: &QT, row: u32, scratch: &mut [f32], w: &mut [f32]) {
    let t = &q.t;
    let g = t.group as usize;
    let ngroup = t.groups() as usize;
    let rowbytes = t.row_bytes() as usize;
    let cb = c.codebook_for(t.bits);
    let p = &q.data[row as usize * rowbytes..];
    let nrm = &q.norms[row as usize * ngroup..];
    let scale = (1.0 / (g as f64).sqrt()) as f32;
    let mask = (1u32 << t.bits) - 1;

    for gi in 0..ngroup {
        let blk = &mut scratch[gi * g..gi * g + g];
        let norm = nrm[gi];
        let bit_off = gi * g * t.bits as usize;
        for (j, slot) in blk.iter_mut().enumerate() {
            let b = bit_off + j * t.bits as usize;
            let byt = b >> 3;
            let sh = b & 7;
            let mut v = p[byt] as u32;
            if sh + t.bits as usize > 8 {
                v |= (p[byt + 1] as u32) << 8;
            }
            *slot = cb[((v >> sh) & mask) as usize] * norm;
        }
        fwht(blk, g);
        for v in blk.iter_mut() {
            *v *= scale;
        }
    }
    let cols = t.shape[1] as usize;
    w[..cols].copy_from_slice(&scratch[..cols]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_ordinary_halves() {
        assert_eq!(f16(0x0000), 0.0);
        assert_eq!(f16(0x8000), -0.0);
        assert_eq!(f16(0x3C00), 1.0);
        assert_eq!(f16(0xBC00), -1.0);
        assert_eq!(f16(0x4000), 2.0);
        assert_eq!(f16(0x3555), 0.333_251_95);
    }

    #[test]
    fn converts_subnormals_and_specials() {
        // Smallest positive subnormal: 2^-24.
        assert_eq!(f16(0x0001), 5.960_464_5e-8);
        assert!(f16(0x7C00).is_infinite() && f16(0x7C00) > 0.0);
        assert!(f16(0xFC00).is_infinite() && f16(0xFC00) < 0.0);
        assert!(f16(0x7E00).is_nan());
    }

    #[test]
    fn the_hadamard_transform_is_its_own_inverse_up_to_scale() {
        let original = [1.0f32, -2.0, 3.0, 0.5, 0.0, 7.0, -1.5, 2.0];
        let mut x = original;
        fwht(&mut x, 8);
        fwht(&mut x, 8);
        for (got, want) in x.iter().zip(original.iter()) {
            assert!((got / 8.0 - want).abs() < 1e-6, "{got} vs {want}");
        }
    }

    #[test]
    fn the_hadamard_transform_of_a_constant_is_a_spike() {
        let mut x = [1.0f32; 4];
        fwht(&mut x, 4);
        assert_eq!(x, [4.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_single_element_transform_is_the_identity() {
        let mut x = [3.0f32];
        fwht(&mut x, 1);
        assert_eq!(x, [3.0]);
    }

    #[test]
    fn the_pair_table_agrees_with_a_direct_dot_product() {
        // The pair table is the subtlest kernel in the port: it depends on the
        // LSB-first packing, so an index-order slip would still produce
        // plausible numbers. Check it against the definition it replaces.
        let cb = [-1.5f32, -0.5, 0.5, 1.5];
        let g = 8u32;
        let xh: Vec<f32> = (0..g).map(|i| i as f32 * 0.5 - 1.0).collect();
        let mut lut = vec![0f32; lut_floats(g)];
        lut_build(&cb, &xh, g, &mut lut);

        // Two bytes hold eight 2-bit indices, low bits first.
        let packed = [0b11_10_01_00u8, 0b00_01_10_11u8];
        let want: f32 = (0..g as usize)
            .map(|j| {
                let idx = (packed[j / 4] >> (2 * (j % 4))) & 3;
                cb[idx as usize] * xh[j]
            })
            .sum();
        let got = dot_group_lut2(&packed, &lut, g);
        assert!((got - want).abs() < 1e-6, "table {got} vs direct {want}");
    }

    #[test]
    fn the_pair_table_is_sized_for_every_pair() {
        assert_eq!(lut_floats(512), 4096);
        assert_eq!(lut_floats(8), 64);
    }
}
