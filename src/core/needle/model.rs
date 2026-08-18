//! The Simple Attention Network forward pass, stepped one token at a time.
//!
//! Weights and session state are deliberately separate types: [`Model`] is
//! immutable after load and [`State`] holds the KV cache, the engram history
//! and every scratch buffer. Each step is `&Model` + `&mut State`, so nothing
//! in the hot path needs to reason about aliasing between weights and buffers.

use super::cact::{Cact, DT_RAW};
use super::quant::{
    dequant_row, f16_slice, fwht, gemv, gemv_gather, gemv_lut2, gemv_rows, lut_build, lut_floats,
    prepare, QT,
};
use super::tokenizer::Tokenizer;

const ND_EPS: f32 = 1e-6;
/// Engram history depth: >= (taps-1)*dilation + 1 = 10, rounded to a power of
/// two so the ring index is a mask-free modulo of a constant.
const EG_HIST: u32 = 16;
const SINKHORN_IT: usize = 20;
const MAX_LANES: usize = 8;
const MAX_SITES: usize = 4;

/// Engram hash constants (`architecture.py`).
const EG_SEED: u32 = 0x9E37_79B9;
const EG_PRIME: u32 = 0x0100_0193;

/// Recent-context slots kept free no matter how long the pinned prefix is.
const MIN_RECENT: u32 = 64;

/// `exp(x)` to ~1e-6 relative, replicating the reference engine's `nd_expf`.
///
/// silu, the attention gate, the engram alpha, Sinkhorn and the attention
/// softmax together make ~30K exponential calls per token. This is *not* an
/// inconsistency with the `f64::exp` in [`Model::pool_cell`]: the reference
/// calls `nd_expf` here and libm `expf` there, and the split is preserved.
///
/// The polynomial is written as separate multiplies and adds on purpose. Go's
/// arm64 backend contracts `p*f + c` into a single FMA, so its results differ
/// from these by 1-5 ULP; this transcribes the C source's operation order
/// instead. Bit-identity across the three engines is not achievable in
/// principle — the ESP32's newlib and macOS's libm already disagree — so the
/// standard is agreement within float32 noise plus identical decisions.
fn fast_exp(x: f32) -> f32 {
    if x > 88.0 {
        return f32::MAX;
    }
    if x < -88.0 {
        return 0.0;
    }
    // Narrows to 0x3fb8aa3b, bit-identical to the reference's 1.44269504.
    let z = x * std::f32::consts::LOG2_E; // x / ln 2
    let k = if z >= 0.0 {
        (z + 0.5) as i32
    } else {
        (z - 0.5) as i32
    };
    let f = z - k as f32;

    let mut p = 0.0013333f32;
    p = p * f + 0.0096181;
    p = p * f + 0.0555041;
    p = p * f + 0.2402265;
    p = p * f + std::f32::consts::LN_2;
    p = p * f + 1.0;

    let scale = f32::from_bits(((k + 127) as u32) << 23); // 2^k
    p * scale
}

fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + fast_exp(-x))
    } else {
        let e = fast_exp(x);
        e / (1.0 + e)
    }
}

/// `x * rsqrt(mean(x^2) + eps)`.
fn rms_unit(x: &[f32], n: usize, out: &mut [f32]) {
    let ss: f32 = x[..n].iter().map(|v| v * v).sum();
    let inv = (1.0 / (ss as f64 / n as f64 + ND_EPS as f64).sqrt()) as f32;
    for (o, v) in out[..n].iter_mut().zip(&x[..n]) {
        *o = v * inv;
    }
}

/// ZCRMSNorm: `(1 + scale) * x / sqrt(mean(x^2) + eps)`.
fn zcrms(scale: &[f32], x: &[f32], n: usize, out: &mut [f32]) {
    let ss: f32 = x[..n].iter().map(|v| v * v).sum();
    let inv = (1.0 / (ss as f64 / n as f64 + ND_EPS as f64).sqrt()) as f32;
    for ((o, v), s) in out[..n].iter_mut().zip(&x[..n]).zip(&scale[..n]) {
        *o = (1.0 + s) * v * inv;
    }
}

/// Per-head ZCRMSNorm, in place, sharing the scale across heads.
fn zcrms_heads(scale: &[f32], x: &mut [f32], nheads: usize, dim: usize) {
    for v in x[..nheads * dim].chunks_mut(dim) {
        let ss: f32 = v.iter().map(|a| a * a).sum();
        let inv = (1.0 / (ss as f64 / dim as f64 + ND_EPS as f64).sqrt()) as f32;
        for (a, s) in v.iter_mut().zip(&scale[..dim]) {
            *a = (1.0 + s) * *a * inv;
        }
    }
}

/// Doubly-stochastic normalisation of an `n x n` matrix, in log space.
fn sinkhorn(a: &mut [f32], n: usize) {
    for _ in 0..SINKHORN_IT {
        for i in 0..n {
            // rows
            let row = &mut a[i * n..i * n + n];
            let mx = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let sum: f32 = row.iter().map(|v| fast_exp(v - mx)).sum();
            let lse = mx + (sum as f64).ln() as f32;
            for v in row.iter_mut() {
                *v -= lse;
            }
        }
        for j in 0..n {
            // columns
            let mut mx = a[j];
            for i in 1..n {
                if a[i * n + j] > mx {
                    mx = a[i * n + j];
                }
            }
            let mut sum = 0f32;
            for i in 0..n {
                sum += fast_exp(a[i * n + j] - mx);
            }
            let lse = mx + (sum as f64).ln() as f32;
            for i in 0..n {
                a[i * n + j] -= lse;
            }
        }
    }
    for v in a[..n * n].iter_mut() {
        *v = fast_exp(*v);
    }
}

/// GPT-NeoX style half-split rotary, against cos/sin precomputed once per token
/// (the position is the same for every layer).
fn apply_rope(cos: &[f32], sin: &[f32], x: &mut [f32], nheads: usize, dim: usize) {
    let half = dim / 2;
    for v in x[..nheads * dim].chunks_mut(dim) {
        for i in 0..half {
            let (c, s) = (cos[i], sin[i]);
            let (x1, x2) = (v[i], v[i + half]);
            v[i] = x1 * c - x2 * s;
            v[i + half] = x2 * c + x1 * s;
        }
    }
}

/// The Hadamard MLP that replaces the standard FFN: diag -> H -> silu -> H ->
/// diag. `out` must hold `next_pow2(d_model)` floats.
fn hadamard_mlp(d1: &[f32], d2: &[f32], d3: &[f32], dm: usize, x: &[f32], out: &mut [f32]) {
    let mut n = 1usize;
    while n < dm {
        n <<= 1;
    }
    let inv = (1.0 / (n as f64).sqrt()) as f32;

    for ((o, a), b) in out[..dm].iter_mut().zip(&d1[..dm]).zip(&x[..dm]) {
        *o = a * b;
    }
    out[dm..n].fill(0.0);
    fwht(&mut out[..n], n);
    for (o, s) in out[..n].iter_mut().zip(&d2[..n]) {
        let z = *o * inv * s;
        *o = z * sigmoid(z); // silu
    }
    fwht(&mut out[..n], n);
    for (o, s) in out[..n].iter_mut().zip(&d3[..n]) {
        *o *= inv * s;
    }
}

/// One block's tensors.
pub struct Layer<'a> {
    q_proj: QT<'a>,
    k_proj: QT<'a>,
    v_proj: QT<'a>,
    gate_proj: QT<'a>,
    out_proj: QT<'a>,

    norm_in: Vec<f32>,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    post_norm: Vec<f32>,
    pre_hada: Vec<f32>,
    d1: Vec<f32>,
    d2: Vec<f32>,
    d3: Vec<f32>,
    attn_gate: f32,
}

/// One engram site's tensors.
struct Engram<'a> {
    tables: QT<'a>,
    key_proj: QT<'a>,
    value_proj: QT<'a>,
    taps: Vec<f32>,
}

/// The immutable half: geometry, weights and the tokenizer.
pub struct Model<'a> {
    c: Cact<'a>,
    pub tok: Tokenizer,

    pub d_model: u32,
    pub n_layers: u32,
    pub n_heads: u32,
    pub n_kv_heads: u32,
    pub head_dim: u32,
    pub attn_dim: u32,
    pub kv_dim: u32,
    pub lanes: u32,
    pub vocab: u32,
    pub window: u32,
    pub n_sites: u32,

    layer: Vec<Layer<'a>>,

    mhc_phi_pre: QT<'a>,
    mhc_phi_post: QT<'a>,
    mhc_phi_res: QT<'a>,
    a_pre: Vec<f32>,
    a_post: Vec<f32>,
    a_res: Vec<f32>,
    b_pre: Vec<f32>,
    b_post: Vec<f32>,
    b_res: Vec<f32>,

    engram: Vec<Engram<'a>>,
    embedding: QT<'a>,
    final_norm: Vec<f32>,

    /// Confidence head (optional; present in `needle2.cact`).
    has_conf: bool,
    conf_proj: Vec<f32>,
    conf_bias: f32,
    n_probes: u32,
    probes: Vec<f32>,

    /// Rotary inverse frequencies: `1/theta^(2i/head_dim)`.
    rope_inv: Vec<f32>,
}

/// The mutable half: KV cache, engram ring, confidence pool and scratch.
pub struct State {
    pub pos: u32,
    n_sink: u32,
    k_cache: Vec<i8>,
    v_cache: Vec<i8>,
    k_scale: Vec<f32>,
    v_scale: Vec<f32>,

    /// Snapshot of session state after a fixed prompt prefix.
    has_snap: bool,
    snap_pos: u32,
    snap_eg_pos: u32,
    snap_hist: [u32; 8],
    snap_eg_hist: Vec<f32>,
    snap_pool_acc: Vec<f32>,
    snap_pool_max: Vec<f32>,
    snap_pool_sum: Vec<f32>,

    /// Recent token ids, for the engram n-grams.
    hist: [u32; 8],
    /// `[site][EG_HIST][d_model]` raw engram v.
    eg_hist: Vec<f32>,
    eg_pos: u32,

    pool_acc: Vec<f32>,
    pool_max: Vec<f32>,
    pool_sum: Vec<f32>,

    // ---- scratch ----
    lane: Vec<f32>,
    lane_next: Vec<f32>,
    nx: Vec<f32>,
    xh: Vec<f32>,
    lut: Vec<f32>,
    u: Vec<f32>,
    ublk: Vec<f32>,
    n1: Vec<f32>,
    n2: Vec<f32>,
    /// The final normalised hidden state, valid after [`Model::step_hidden`].
    pub y: Vec<f32>,
    tmp: Vec<f32>,
    tmp2: Vec<f32>,
    q: Vec<f32>,
    kbuf: Vec<f32>,
    vbuf: Vec<f32>,
    gate: Vec<f32>,
    attn: Vec<f32>,
    aout: Vec<f32>,
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
    eg_k: Vec<f32>,
    eg_v: Vec<f32>,
    pub logits: Vec<f32>,
    row: Vec<f32>,
}

impl<'a> Model<'a> {
    /// Open a model over a `.cact` blob, used in place.
    pub fn new(blob: &'a [u8]) -> Result<Self, &'static str> {
        let c = Cact::open(blob)?;
        let h = c.h.clone();

        let lanes = h.mhc_lanes;
        let n_sites = h.num_sites;
        if lanes as usize > MAX_LANES || n_sites as usize > MAX_SITES {
            return Err("model: lanes/sites out of range");
        }
        let window = if h.kv_window == 0 {
            h.max_seq_len
        } else {
            h.kv_window
        };

        // Canonical positional layout, verified against the shipped blob.
        // Tensor binding is positional — `.cact` carries no names — so an
        // off-by-one here silently produces garbage rather than an error.
        let embedding = QT::new(&c, 0);

        let mut layer = Vec::with_capacity(h.num_layers as usize);
        for i in 0..h.num_layers {
            // norm_in, q, k, v, q_norm, k_norm, gate, out, post_norm,
            // attn_gate, pre_hada, d1, d2, d3.
            let b = 1 + i * 14;
            let f16 = |k: u32| {
                let t = c.tensor_at(b + k);
                f16_slice(&c, &t)
            };
            let attn_gate = *f16(9).first().ok_or("model: empty attn_gate")?;
            layer.push(Layer {
                norm_in: f16(0),
                q_proj: QT::new(&c, b + 1),
                k_proj: QT::new(&c, b + 2),
                v_proj: QT::new(&c, b + 3),
                q_norm: f16(4),
                k_norm: f16(5),
                gate_proj: QT::new(&c, b + 6),
                out_proj: QT::new(&c, b + 7),
                post_norm: f16(8),
                attn_gate,
                pre_hada: f16(10),
                d1: f16(11),
                d2: f16(12),
                d3: f16(13),
            });
        }

        let mut base = 1 + h.num_layers * 14;
        let mhc = |k: u32| {
            let t = c.tensor_at(base + k);
            f16_slice(&c, &t)
        };
        let (a_pre, a_post, a_res) = (mhc(0), mhc(1), mhc(2));
        let (b_pre, b_post, b_res) = (mhc(3), mhc(4), mhc(5));
        let mhc_phi_pre = QT::new(&c, base + 6);
        let mhc_phi_post = QT::new(&c, base + 7);
        let mhc_phi_res = QT::new(&c, base + 8);
        base += 9;

        let mut engram = Vec::with_capacity(n_sites as usize);
        for s in 0..n_sites {
            let taps_t = c.tensor_at(base + s * 4 + 3);
            engram.push(Engram {
                tables: QT::new(&c, base + s * 4),
                key_proj: QT::new(&c, base + s * 4 + 1),
                value_proj: QT::new(&c, base + s * 4 + 2),
                taps: f16_slice(&c, &taps_t),
            });
        }
        base += n_sites * 4;
        let fn_t = c.tensor_at(base);
        let final_norm = f16_slice(&c, &fn_t);

        // Probe heads, if this blob carries them. Layout after final_norm: a
        // manifest of H head codes (1 contrastive, 2 confidence), then H fixed
        // triples [probes, proj, bias]; the tokenizer is the final tensor.
        let after = base + 1;
        let mut has_conf = false;
        let mut conf_proj = Vec::new();
        let mut conf_bias = 0f32;
        let mut n_probes = 0u32;
        let mut probes_f = Vec::new();
        let extra = c.n.saturating_sub(after + 1);
        if extra >= 4 {
            let manifest = c.tensor_at(after);
            let man_d = f16_slice(&c, &manifest);
            let heads = (extra - 1) / 3;
            for k in 0..heads.min(manifest.shape[0]) {
                let code = man_d[k as usize];
                if code > 1.5 && code < 2.5 {
                    // confidence
                    let probes = c.tensor_at(after + 1 + k * 3);
                    let proj = c.tensor_at(after + 1 + k * 3 + 1);
                    let bias = c.tensor_at(after + 1 + k * 3 + 2);
                    // proj is (1, n_probes*d_model); probes is (P, d_model).
                    if probes.shape[1] == h.d_model && proj.shape[1] == probes.shape[0] * h.d_model
                    {
                        conf_proj = f16_slice(&c, &proj);
                        conf_bias = f16_slice(&c, &bias)[0];
                        n_probes = probes.shape[0];
                        // Expand the probes once; pool_cell would otherwise
                        // convert P*d_model halves per hidden cell.
                        probes_f = f16_slice(&c, &probes);
                        has_conf = true;
                    }
                }
            }
        }

        // The tokenizer is the single RAW tensor, last in canon order.
        let mut tok = None;
        for i in (0..c.n).rev() {
            let t = c.tensor_at(i);
            if t.dtype == DT_RAW {
                tok = c.data(&t).and_then(|d| Tokenizer::new(d).ok());
                break;
            }
        }
        let tok = tok.ok_or("model: blob carries no tokenizer")?;

        let rope_inv = (0..h.head_dim / 2)
            .map(|i| (1.0 / (h.rope_theta as f64).powf(2.0 * i as f64 / h.head_dim as f64)) as f32)
            .collect();

        Ok(Model {
            d_model: h.d_model,
            n_layers: h.num_layers,
            n_heads: h.num_heads,
            n_kv_heads: h.num_kv_heads,
            head_dim: h.head_dim,
            attn_dim: h.attn_dim,
            kv_dim: h.num_kv_heads * h.head_dim,
            lanes,
            vocab: h.vocab_size,
            window,
            n_sites,
            layer,
            mhc_phi_pre,
            mhc_phi_post,
            mhc_phi_res,
            a_pre,
            a_post,
            a_res,
            b_pre,
            b_post,
            b_res,
            engram,
            embedding,
            final_norm,
            has_conf,
            conf_proj,
            conf_bias,
            n_probes,
            probes: probes_f,
            rope_inv,
            tok,
            c,
        })
    }

    /// Allocate the session buffers for this geometry.
    pub fn new_state(&self) -> State {
        let dm = self.d_model as usize;
        let nl = self.lanes as usize * dm;
        let mut dpow = 1usize;
        while dpow < dm {
            dpow <<= 1;
        }
        let xh_n = nl.max(dm);
        let kvn = self.n_layers as usize * self.window as usize * self.kv_dim as usize;
        let scn = self.n_layers as usize * self.window as usize * self.n_kv_heads as usize;
        let probes = self.n_probes as usize;

        let mut st = State {
            pos: 0,
            n_sink: 0,
            k_cache: vec![0; kvn],
            v_cache: vec![0; kvn],
            k_scale: vec![0.0; scn],
            v_scale: vec![0.0; scn],
            has_snap: false,
            snap_pos: 0,
            snap_eg_pos: 0,
            snap_hist: [0; 8],
            snap_eg_hist: Vec::new(),
            snap_pool_acc: Vec::new(),
            snap_pool_max: Vec::new(),
            snap_pool_sum: Vec::new(),
            hist: [0; 8],
            eg_hist: vec![0.0; self.n_sites as usize * EG_HIST as usize * dm],
            eg_pos: 0,
            pool_acc: vec![0.0; if self.has_conf { probes * dm } else { 0 }],
            pool_max: vec![0.0; if self.has_conf { probes } else { 0 }],
            pool_sum: vec![0.0; if self.has_conf { probes } else { 0 }],
            lane: vec![0.0; nl],
            lane_next: vec![0.0; nl],
            nx: vec![0.0; nl],
            xh: vec![0.0; xh_n],
            lut: vec![0.0; lut_floats(self.d_model)],
            u: vec![0.0; dm],
            ublk: vec![0.0; dm],
            n1: vec![0.0; dm],
            n2: vec![0.0; dpow],
            y: vec![0.0; dm],
            tmp: vec![0.0; dm],
            tmp2: vec![0.0; xh_n],
            q: vec![0.0; self.attn_dim as usize],
            kbuf: vec![0.0; self.kv_dim as usize],
            vbuf: vec![0.0; self.kv_dim as usize],
            gate: vec![0.0; self.attn_dim as usize],
            attn: vec![0.0; self.attn_dim as usize],
            aout: vec![0.0; dm],
            rope_cos: vec![0.0; self.head_dim as usize / 2],
            rope_sin: vec![0.0; self.head_dim as usize / 2],
            eg_k: vec![0.0; self.n_sites as usize * dm],
            eg_v: vec![0.0; self.n_sites as usize * dm],
            logits: vec![0.0; self.vocab as usize],
            row: vec![0.0; dm.max(128)],
        };
        self.reset(&mut st);
        st
    }

    /// Clear the KV cache and conversation position.
    pub fn reset(&self, st: &mut State) {
        st.pos = 0;
        st.n_sink = 0;
        st.has_snap = false;
        st.eg_pos = 0;
        st.pool_acc.fill(0.0);
        st.pool_max.fill(f32::NEG_INFINITY);
        st.pool_sum.fill(0.0);
        st.hist = [0; 8];
        st.eg_hist.fill(0.0);
    }

    /// Pin the first `n` positions so the sliding window cannot evict them.
    ///
    /// Needle renders the tool schemas at the head of the prompt and relies on
    /// them staying visible for the whole turn; without pinning, a long prompt
    /// plus a long generation scrolls the `<tools>` block out and the model
    /// starts inventing tool names. `n` is clamped to leave recent context.
    fn set_sink(&self, st: &mut State, n: u32) {
        let cap = self.window.saturating_sub(MIN_RECENT);
        st.n_sink = n.min(cap);
    }

    /// Freeze the current position as a reusable prefix: pin the KV sink and
    /// snapshot the engram and confidence state.
    pub fn snapshot(&self, st: &mut State) {
        self.set_sink(st, st.pos);
        st.snap_pos = st.pos;
        st.snap_eg_pos = st.eg_pos;
        st.snap_hist = st.hist;
        st.snap_eg_hist = st.eg_hist.clone();
        st.snap_pool_acc = st.pool_acc.clone();
        st.snap_pool_max = st.pool_max.clone();
        st.snap_pool_sum = st.pool_sum.clone();
        st.has_snap = true;
    }

    /// Resume from the snapshot, discarding everything decoded since.
    pub fn rewind(&self, st: &mut State) {
        if !st.has_snap {
            self.reset(st);
            return;
        }
        st.pos = st.snap_pos;
        st.eg_pos = st.snap_eg_pos;
        st.hist = st.snap_hist;
        st.eg_hist.copy_from_slice(&st.snap_eg_hist);
        st.pool_acc.copy_from_slice(&st.snap_pool_acc);
        st.pool_max.copy_from_slice(&st.snap_pool_max);
        st.pool_sum.copy_from_slice(&st.snap_pool_sum);
    }

    /// Map an absolute position to a cache slot. Sinks occupy the first
    /// `n_sink` slots permanently; everything after rings through the rest.
    fn kv_slot(&self, st: &State, p: u32) -> u32 {
        if p < st.n_sink {
            p
        } else {
            st.n_sink + (p - st.n_sink) % (self.window - st.n_sink)
        }
    }

    /// The calibrated confidence over everything fed so far, in `[0,1]`.
    /// Returns `-1` if the blob carries no confidence head.
    ///
    /// This is a groundedness score: how well the arguments are evidenced in
    /// the request, not whether the call is correct.
    pub fn confidence(&self, st: &State) -> f32 {
        if !self.has_conf {
            return -1.0;
        }
        let dm = self.d_model as usize;
        let mut logit = self.conf_bias;
        for k in 0..self.n_probes as usize {
            let ac = &st.pool_acc[k * dm..(k + 1) * dm];
            let inv = if st.pool_sum[k] > 0.0 {
                1.0 / st.pool_sum[k]
            } else {
                0.0
            };
            for (p, a) in self.conf_proj[k * dm..(k + 1) * dm].iter().zip(ac) {
                logit += p * a * inv;
            }
        }
        sigmoid(logit)
    }

    /// Fold one hidden cell into the running softmax pool.
    ///
    /// `probe_pool()` softmaxes `probe.cell/sqrt(d)` over every cell of every
    /// token, then takes the weighted mean. Streaming it with a running max
    /// keeps the result identical to the batch computation while holding only
    /// the accumulator.
    fn pool_cell(&self, st: &mut State, cell: &[f32]) {
        let dm = self.d_model as usize;
        let inv_s = (1.0 / (dm as f64).sqrt()) as f32;
        for k in 0..self.n_probes as usize {
            let pr = &self.probes[k * dm..k * dm + dm];
            let z: f32 = pr.iter().zip(&cell[..dm]).map(|(p, c)| p * c).sum::<f32>() * inv_s;

            let ac = &mut st.pool_acc[k * dm..k * dm + dm];
            if z > st.pool_max[k] {
                let rescale = ((st.pool_max[k] - z) as f64).exp() as f32;
                for v in ac.iter_mut() {
                    *v *= rescale;
                }
                st.pool_sum[k] *= rescale;
                st.pool_max[k] = z;
            }
            // libm `expf` in the reference, not nd_expf — keep the split.
            let w = ((z - st.pool_max[k]) as f64).exp() as f32;
            st.pool_sum[k] += w;
            for (a, c) in ac.iter_mut().zip(&cell[..dm]) {
                *a += w * c;
            }
        }
    }

    /// Compute k/v for the current token at every engram site.
    fn engram_step(&self, st: &mut State, token: u32) {
        let h = &self.c.h;
        let orders = h.num_orders;
        let heads = h.engram_tables / orders.max(1);
        let slots = h.engram_slots;
        let sub = h.engram_sub_dim as usize;
        let dil = h.engram_dilation;
        let taps = h.engram_conv_taps;
        let dm = self.d_model as usize;

        // Shift the token history: hist[0] is the current token.
        for j in (1..8).rev() {
            st.hist[j] = st.hist[j - 1];
        }
        st.hist[0] = token;

        for s in 0..self.n_sites as usize {
            let eg = &self.engram[s];
            let mut table = 0u32;

            for oi in 0..orders {
                let order = h.orders[oi as usize];
                for hh in 0..heads {
                    // Wrapping throughout: this is a hash, and Go's uint32
                    // arithmetic wraps where Rust would panic in debug.
                    let seed = EG_SEED.wrapping_mul(oi * heads + hh + 1);
                    let mut acc = seed;
                    let ok = st.pos + 1 >= order; // enough history

                    for j in 0..order {
                        let tk = if j <= st.pos && (j as usize) < st.hist.len() {
                            st.hist[j as usize]
                        } else {
                            0
                        };
                        acc = (acc ^ tk).wrapping_mul(EG_PRIME);
                    }
                    acc ^= acc >> 15;
                    let idx = acc % slots;

                    // The engram vector reuses xh, which is >= d_model long.
                    let lo = table as usize * sub;
                    if ok {
                        // Split the borrow: dequant_row writes into xh through
                        // `dst` while `row` is a separate scratch buffer.
                        let (row, dst) = (&mut st.row, &mut st.xh[lo..lo + sub]);
                        dequant_row(&self.c, &eg.tables, table * slots + idx, row, dst);
                    } else {
                        st.xh[lo..lo + sub].fill(0.0);
                    }
                    table += 1;
                }
            }

            // k = key_proj @ e, raw v = value_proj @ e.
            prepare(&eg.key_proj.t, &st.xh, &mut st.tmp2);
            lut_build(
                self.c.codebook_for(2),
                &st.tmp2,
                eg.key_proj.t.in_pad(),
                &mut st.lut,
            );
            gemv_lut2(&eg.key_proj, &st.lut, &mut st.eg_k[s * dm..s * dm + dm]);
            let write = (s * EG_HIST as usize + (st.eg_pos % EG_HIST) as usize) * dm;
            gemv_lut2(&eg.value_proj, &st.lut, &mut st.eg_hist[write..write + dm]);

            // Dilated causal tap convolution over the raw v history.
            st.eg_v[s * dm..s * dm + dm].fill(0.0);
            for j in 0..taps {
                let back = j * dil;
                if back > st.pos {
                    continue; // tap_ok
                }
                let src = (s * EG_HIST as usize
                    + ((st.eg_pos + EG_HIST - back % EG_HIST) % EG_HIST) as usize)
                    * dm;
                let tap = &eg.taps[j as usize * dm..j as usize * dm + dm];
                let (out, hist) = (
                    &mut st.eg_v[s * dm..s * dm + dm],
                    &st.eg_hist[src..src + dm],
                );
                for ((o, t), v) in out.iter_mut().zip(tap).zip(hist) {
                    *o += t * v;
                }
            }
        }
        st.eg_pos += 1;
    }

    /// One layer's gated GQA with an online softmax, reading `st.n1` and
    /// writing `st.aout`.
    ///
    /// A two-pass max-then-accumulate computes every `q.k` twice, and with a
    /// pinned prefix those dot products dominate the whole forward pass. A
    /// running max with a rescaled accumulator gives the identical result in
    /// one pass.
    fn attention(&self, st: &mut State, li: u32) {
        let l = &self.layer[li as usize];
        let (hd, nh, nkv) = (
            self.head_dim as usize,
            self.n_heads as usize,
            self.n_kv_heads as usize,
        );
        let slot = self.kv_slot(st, st.pos);
        let kvbase = (li as usize * self.window as usize + slot as usize) * self.kv_dim as usize;
        let scbase = (li as usize * self.window as usize + slot as usize) * nkv;

        // q, k, v and the gate all reduce over the same activation, so one
        // prepare and one pair table serve all four.
        prepare(&l.q_proj.t, &st.n1, &mut st.xh);
        lut_build(
            self.c.codebook_for(2),
            &st.xh,
            l.q_proj.t.in_pad(),
            &mut st.lut,
        );
        gemv_lut2(&l.q_proj, &st.lut, &mut st.q);
        gemv_lut2(&l.k_proj, &st.lut, &mut st.kbuf);
        gemv_lut2(&l.v_proj, &st.lut, &mut st.vbuf);
        gemv_lut2(&l.gate_proj, &st.lut, &mut st.gate);

        zcrms_heads(&l.q_norm, &mut st.q, nh, hd);
        zcrms_heads(&l.k_norm, &mut st.kbuf, nkv, hd);

        apply_rope(&st.rope_cos, &st.rope_sin, &mut st.q, nh, hd);
        apply_rope(&st.rope_cos, &st.rope_sin, &mut st.kbuf, nkv, hd);

        // Store this position's k/v as symmetric int8, one scale per head.
        for kh in 0..nkv {
            let mut mk = 0f32;
            let mut mv = 0f32;
            for i in 0..hd {
                mk = mk.max(st.kbuf[kh * hd + i].abs());
                mv = mv.max(st.vbuf[kh * hd + i].abs());
            }
            let ks = if mk > 0.0 { mk / 127.0 } else { 1.0 };
            let vs = if mv > 0.0 { mv / 127.0 } else { 1.0 };
            st.k_scale[scbase + kh] = ks;
            st.v_scale[scbase + kh] = vs;
            for i in 0..hd {
                let kq = (st.kbuf[kh * hd + i] / ks).clamp(-127.0, 127.0);
                let vq = (st.vbuf[kh * hd + i] / vs).clamp(-127.0, 127.0);
                // lrintf, i.e. round-half-to-EVEN. Rounding half away from zero
                // here flipped one generation in ten before it was caught, and
                // the unit tests did not see it.
                st.k_cache[kvbase + kh * hd + i] = (kq as f64).round_ties_even() as i8;
                st.v_cache[kvbase + kh * hd + i] = (vq as f64).round_ties_even() as i8;
            }
        }

        // Attend to the pinned sinks [0, n_sink) plus the most recent
        // (window - n_sink) positions. With n_sink == 0 this is a plain sliding
        // window; the two runs are visited without materialising a score
        // buffer.
        let rep = nh / nkv;
        let scale = (1.0 / (hd as f64).sqrt()) as f32;
        let rcap = self.window - st.n_sink;
        let sinks = st.n_sink.min(st.pos + 1);
        let (rfirst, rcount) = if st.pos < st.n_sink {
            (st.pos + 1, 0)
        } else {
            let rcount = (st.pos + 1 - st.n_sink).min(rcap);
            (st.pos + 1 - rcount, rcount)
        };

        for h in 0..nh {
            let kvh = h / rep;
            let mut mx = f32::NEG_INFINITY;
            let mut denom = 0f32;
            st.attn[h * hd..h * hd + hd].fill(0.0);

            for run in 0..2 {
                let (base, count) = if run == 0 {
                    (0, sinks)
                } else {
                    (rfirst, rcount)
                };
                for p in 0..count {
                    let sl = self.kv_slot(st, base + p) as usize;
                    let kb =
                        (li as usize * self.window as usize + sl) * self.kv_dim as usize + kvh * hd;
                    let sb = (li as usize * self.window as usize + sl) * nkv + kvh;

                    let mut dot = 0f32;
                    for i in 0..hd {
                        dot += st.q[h * hd + i] * st.k_cache[kb + i] as f32;
                    }
                    dot *= st.k_scale[sb] * scale;

                    if dot > mx {
                        if denom > 0.0 {
                            let rescale = fast_exp(mx - dot);
                            for o in st.attn[h * hd..h * hd + hd].iter_mut() {
                                *o *= rescale;
                            }
                            denom *= rescale;
                        }
                        mx = dot;
                    }
                    let w = fast_exp(dot - mx);
                    denom += w;
                    let wv = w * st.v_scale[sb];
                    let (oh, vp) = (&mut st.attn[h * hd..h * hd + hd], &st.v_cache[kb..kb + hd]);
                    for (o, v) in oh.iter_mut().zip(vp) {
                        *o += wv * *v as f32;
                    }
                }
            }
            let inv = 1.0 / denom;
            for o in st.attn[h * hd..h * hd + hd].iter_mut() {
                *o *= inv;
            }
        }

        // Gate, then project back to d_model.
        for (a, g) in st.attn.iter_mut().zip(&st.gate) {
            *a *= sigmoid(*g);
        }
        prepare(&l.out_proj.t, &st.attn, &mut st.xh);
        lut_build(
            self.c.codebook_for(2),
            &st.xh,
            l.out_proj.t.in_pad(),
            &mut st.lut,
        );
        gemv_lut2(&l.out_proj, &st.lut, &mut st.aout);
    }

    /// engram-inject -> ZCRMSNorm -> gated GQA -> ZCRMSNorm -> gated residual
    /// -> ZCRMSNorm -> Hadamard MLP -> residual, in place on `st.u`.
    fn block(&self, st: &mut State, li: u32) {
        let dm = self.d_model as usize;

        for s in 0..self.n_sites as usize {
            if self.c.h.sites[s] != li {
                continue;
            }
            rms_unit(&st.u, dm, &mut st.n1);
            let (ek, n2) = (&st.eg_k[s * dm..s * dm + dm], &mut st.n2);
            rms_unit(ek, dm, n2);
            let dot: f32 = st.n1[..dm]
                .iter()
                .zip(&st.n2[..dm])
                .map(|(a, b)| a * b)
                .sum();
            let alpha = sigmoid(dot / (dm as f64).sqrt() as f32);
            let (u, ev) = (&mut st.u, &st.eg_v[s * dm..s * dm + dm]);
            for (a, v) in u.iter_mut().zip(ev) {
                *a += alpha * v;
            }
        }

        // Attention sub-block.
        zcrms(&self.layer[li as usize].norm_in, &st.u, dm, &mut st.n1);
        self.attention(st, li);
        let l = &self.layer[li as usize];
        zcrms(&l.post_norm, &st.aout, dm, &mut st.n2);
        let g = sigmoid(l.attn_gate);
        for (a, v) in st.u.iter_mut().zip(&st.n2[..dm]) {
            *a += g * v;
        }

        // Hadamard MLP sub-block. The MLP writes into a padded buffer, so n2
        // must hold next_pow2(d_model) floats.
        zcrms(&l.pre_hada, &st.u, dm, &mut st.n1);
        hadamard_mlp(&l.d1, &l.d2, &l.d3, dm, &st.n1, &mut st.n2);
        for (a, v) in st.u.iter_mut().zip(&st.n2[..dm]) {
            *a += v;
        }
    }

    /// Feed one token; the final normalised hidden state lands in `st.y`.
    ///
    /// Stops before the vocabulary projection: prefill never looks at logits,
    /// and constrained decoding only needs a handful of rows.
    pub fn step_hidden(&self, st: &mut State, token: u32) {
        let dm = self.d_model as usize;
        let n = self.lanes as usize;
        let nl = n * dm;
        let escale = (dm as f64).sqrt() as f32;

        // Rotary tables for this position, shared by every layer.
        for i in 0..self.head_dim as usize / 2 {
            let angle = st.pos as f64 * self.rope_inv[i] as f64;
            st.rope_cos[i] = angle.cos() as f32;
            st.rope_sin[i] = angle.sin() as f32;
        }

        // Embedding (tied), scaled.
        let (row, tmp) = (&mut st.row, &mut st.tmp);
        dequant_row(&self.c, &self.embedding, token, row, tmp);
        for v in st.tmp[..dm].iter_mut() {
            *v *= escale;
        }
        if self.has_conf {
            let cell = std::mem::take(&mut st.tmp);
            self.pool_cell(st, &cell); // cells[0] = x0
            st.tmp = cell;
        }
        for j in 0..n {
            st.lane[j * dm..j * dm + dm].copy_from_slice(&st.tmp[..dm]);
        }

        self.engram_step(st, token);

        let mut hpre = [0f32; MAX_LANES];
        let mut hpost = [0f32; MAX_LANES];
        let mut hres = [0f32; MAX_LANES * MAX_LANES];

        for li in 0..self.n_layers {
            let a_pre = self.a_pre[li as usize];
            let a_post = self.a_post[li as usize];
            let a_res = self.a_res[li as usize];
            let lane_id = li as usize % n;

            rms_unit(&st.lane, nl, &mut st.nx);

            // The phi tensors stack all layers; this layer owns a row slice.
            prepare(&self.mhc_phi_pre.t, &st.nx, &mut st.xh);
            let nu = n as u32;
            gemv_rows(&self.c, &self.mhc_phi_pre, &st.xh, li * nu, nu, &mut hpre);
            gemv_rows(&self.c, &self.mhc_phi_post, &st.xh, li * nu, nu, &mut hpost);
            gemv_rows(
                &self.c,
                &self.mhc_phi_res,
                &st.xh,
                li * nu * nu,
                nu * nu,
                &mut hres,
            );

            let (bpre, bpost) = (
                &self.b_pre[li as usize * n..],
                &self.b_post[li as usize * n..],
            );
            for (j, (pre, post)) in hpre[..n].iter_mut().zip(&mut hpost[..n]).enumerate() {
                // The layer's own lane is biased open, the others closed.
                let (pre_off, post_off) = if j == lane_id {
                    (8.0 - 4.0, 0.0)
                } else {
                    (-4.0, -4.0)
                };
                *pre = sigmoid(a_pre * *pre + bpre[j] + pre_off);
                *post = 2.0 * sigmoid(a_post * *post + bpost[j] + post_off);
            }
            for (i, v) in hres[..n * n].iter_mut().enumerate() {
                *v = a_res * *v + self.b_res[li as usize * n * n + i];
            }
            sinkhorn(&mut hres, n);

            // u = sum_j hpre[j] * lane[j], accumulated lane by lane so both
            // sides are walked sequentially.
            st.u[..dm].fill(0.0);
            for (h, lane) in hpre[..n].iter().zip(st.lane.chunks_exact(dm)) {
                for (u, l) in st.u[..dm].iter_mut().zip(lane) {
                    *u += h * l;
                }
            }

            // y = block(u) - u
            st.ublk[..dm].copy_from_slice(&st.u[..dm]);
            self.block(st, li);
            for (u, b) in st.u.iter_mut().zip(&st.ublk[..dm]) {
                *u -= b;
            }

            // lane' = hres @ lane + hpost * y
            for j in 0..n {
                for i in 0..dm {
                    let mut acc = 0f32;
                    for k in 0..n {
                        acc += hres[j * n + k] * st.lane[k * dm + i];
                    }
                    st.lane_next[j * dm + i] = acc + hpost[j] * st.u[i];
                }
            }
            std::mem::swap(&mut st.lane, &mut st.lane_next);

            if self.has_conf {
                // collect_hidden yields the mean over lanes for each layer.
                for i in 0..dm {
                    let mut acc = 0f32;
                    for j in 0..n {
                        acc += st.lane[j * dm + i];
                    }
                    st.n1[i] = acc / n as f32;
                }
                let cell = std::mem::take(&mut st.n1);
                self.pool_cell(st, &cell);
                st.n1 = cell;
            }
        }

        // Mean over lanes, final norm.
        for i in 0..dm {
            let mut acc = 0f32;
            for j in 0..n {
                acc += st.lane[j * dm + i];
            }
            st.tmp[i] = acc / n as f32;
        }
        zcrms(&self.final_norm, &st.tmp, dm, &mut st.y);

        st.pos += 1;
    }

    /// Project the hidden state through the tied embedding into `st.logits`.
    pub fn logits_all(&self, st: &mut State) {
        let (y, xh, logits) = (&st.y, &mut st.xh, &mut st.logits);
        gemv(&self.c, &self.embedding, y, xh, logits);
    }

    /// Score only the given token ids.
    pub fn logits_subset(&self, st: &mut State, ids: &[u32], out: &mut [f32]) {
        prepare(&self.embedding.t, &st.y, &mut st.xh);
        gemv_gather(&self.c, &self.embedding, &st.xh, ids, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_exp_tracks_the_real_exponential() {
        for x in [-10.0f32, -1.0, -0.5, 0.0, 0.5, 1.0, 5.0, 20.0] {
            let want = (x as f64).exp() as f32;
            let got = fast_exp(x);
            assert!(
                ((got - want) / want).abs() < 5e-6,
                "exp({x}): got {got}, want {want}"
            );
        }
    }

    #[test]
    fn fast_exp_saturates_instead_of_overflowing() {
        assert_eq!(fast_exp(100.0), f32::MAX);
        assert_eq!(fast_exp(-100.0), 0.0);
        // -88 is inside the guard but the 2^k scale underflows to zero there
        // (k == -127 makes the exponent field 0). The reference does the same;
        // softmax only ever sees `x - max <= 0`, so this tail is unreachable in
        // practice.
        assert_eq!(fast_exp(-88.0), 0.0);
    }

    #[test]
    fn sigmoid_is_symmetric_about_a_half() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-7);
        for x in [0.5f32, 2.0, 7.0] {
            assert!((sigmoid(x) + sigmoid(-x) - 1.0).abs() < 1e-6, "at {x}");
        }
        assert!(sigmoid(50.0) > 0.999_99);
        assert!(sigmoid(-50.0) < 1e-5);
    }

    #[test]
    fn rms_unit_normalises_to_unit_mean_square() {
        let x = [3.0f32, -4.0, 0.0, 5.0];
        let mut out = [0f32; 4];
        rms_unit(&x, 4, &mut out);
        let ms: f32 = out.iter().map(|v| v * v).sum::<f32>() / 4.0;
        assert!((ms - 1.0).abs() < 1e-4, "mean square was {ms}");
    }

    #[test]
    fn zcrms_applies_the_scale_on_top_of_the_norm() {
        let x = [1.0f32, 2.0, 3.0, 4.0];
        let scale = [0.0f32, 1.0, 0.0, 0.0];
        let (mut plain, mut scaled) = ([0f32; 4], [0f32; 4]);
        rms_unit(&x, 4, &mut plain);
        zcrms(&scale, &x, 4, &mut scaled);
        assert!((scaled[0] - plain[0]).abs() < 1e-6);
        assert!((scaled[1] - 2.0 * plain[1]).abs() < 1e-6);
    }

    #[test]
    fn sinkhorn_produces_a_doubly_stochastic_matrix() {
        let mut a = [0.3f32, -1.2, 0.7, 2.0, 0.1, -0.4, 1.1, 0.0, 0.5];
        sinkhorn(&mut a, 3);
        for i in 0..3 {
            let row: f32 = (0..3).map(|j| a[i * 3 + j]).sum();
            let col: f32 = (0..3).map(|j| a[j * 3 + i]).sum();
            assert!((row - 1.0).abs() < 1e-3, "row {i} summed to {row}");
            assert!((col - 1.0).abs() < 1e-3, "col {i} summed to {col}");
        }
    }

    #[test]
    fn rope_preserves_the_norm_of_each_head() {
        let cos = [0.6f32, 0.8];
        let sin = [0.8f32, 0.6];
        let mut x = [1.0f32, 2.0, 3.0, 4.0];
        let before: f32 = x.iter().map(|v| v * v).sum();
        apply_rope(&cos, &sin, &mut x, 1, 4);
        let after: f32 = x.iter().map(|v| v * v).sum();
        assert!((before - after).abs() < 1e-4);
    }
}
