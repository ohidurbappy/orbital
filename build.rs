//! Fetch the model weights so the binary can embed them.
//!
//! `orbital do` runs a 45M-parameter model that ships *inside* the executable,
//! so nothing has to be downloaded, cached or found on disk at runtime. The
//! blob is far too large to keep in git, so it is fetched once into `model/`
//! (git-ignored) and checked against its SHA-256 before any build embeds it.
//!
//! The path is deliberately the repo, not `OUT_DIR`: `cargo clean` would
//! otherwise throw away 13.7 MB that never changes.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MODEL_URL: &str =
    "https://huggingface.co/Cactus-Compute/needle2/resolve/main/needle2.cact?download=true";
const MODEL_SHA256: &str = "b43aabfcaf1a6db6acf488076eab71d823c08697c7af4521fc1d174b60ede5ba";
const MODEL_BYTES: u64 = 13_737_807;

fn main() {
    // CI stamps the release version through the environment (see
    // src/core/version.rs), so a cached build must be redone when it changes.
    println!("cargo:rerun-if-env-changed=ORBITAL_VERSION");
    println!("cargo:rerun-if-changed=build.rs");

    let path = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("model")
        .join("needle2.cact");
    println!("cargo:rerun-if-changed={}", path.display());

    if is_current(&path) {
        return;
    }
    if let Err(err) = fetch(&path) {
        // A half-written file would fail the checksum forever after.
        let _ = fs::remove_file(&path);
        panic!(
            "could not fetch the model weights: {err}\n\
             \n\
             `orbital do` embeds a 13.7 MB model. Download it once with:\n\
             \n  curl -fL -o {} \"{MODEL_URL}\"\n",
            path.display()
        );
    }
}

/// Whether the blob on disk is present, the right size, and the right bytes.
fn is_current(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    // Cheap gate first: the hash costs ~30 ms and runs on every build.
    if meta.len() != MODEL_BYTES {
        return false;
    }
    match fs::read(path) {
        Ok(bytes) => hex(&sha256(&bytes)) == MODEL_SHA256,
        Err(_) => false,
    }
}

fn fetch(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    println!("cargo:warning=downloading needle2 weights (13.7 MB, one time)");

    let mut body = Vec::with_capacity(MODEL_BYTES as usize);
    ureq::get(MODEL_URL)
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .into_reader()
        .take(MODEL_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;

    let got = hex(&sha256(&body));
    if got != MODEL_SHA256 {
        return Err(format!(
            "checksum mismatch: got {got}, expected {MODEL_SHA256}"
        ));
    }
    fs::write(path, &body).map_err(|e| e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 (FIPS 180-4), inlined so the build needs no extra dependency.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = [0u8; 32];
    for (chunk, v) in out.chunks_mut(4).zip(h) {
        chunk.copy_from_slice(&v.to_be_bytes());
    }
    out
}
