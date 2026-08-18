//! A Rust port of the Needle 2 inference engine — a 45M-parameter dispatcher
//! that turns a sentence into a grammar-guaranteed tool call.
//!
//! Ported from the Go engine in `needle-2-test/needle/`, itself a port of the
//! portable C99 reference that runs the same `.cact` blob on an ESP32-S3. The
//! source of truth for intended behaviour is that C implementation.
//!
//! The geometry is read from the blob header, not baked in: for the shipped
//! model that is 27 layers, `d_model` 512, 8 query heads over 4 KV heads, vocab
//! 8192, a 256-token sliding window, 4 hyper-connection lanes, and engram sites
//! at layers 2 and 15.
//!
//! Three invariants are easy to break and produce silently wrong output rather
//! than an error:
//!
//! * **Tensor binding is positional.** `.cact` carries no tensor names, so the
//!   order in [`model::Model::new`] *is* the format.
//! * **The schema must be compacted** before it reaches the model — see
//!   [`grammar::json_compact`].
//! * **The int8 KV cache rounds half to even**, matching C's `lrintf`.
//!
//! Everything here is dependency-free and does no I/O, so it is testable
//! without a model file.

pub mod cact;
pub mod grammar;
pub mod model;
pub mod parse;
pub mod quant;
pub mod sample;
pub mod session;
pub mod tokenizer;
