//! The stages of the diarization pipeline that need the optional `diarization`
//! dependencies: ONNX Runtime and ndarray (segmentation, embedding), rustfft
//! (the filterbank the embedder reads) and kodama (clustering).
//!
//! Everything in `diarization/` outside this directory is plain Rust and
//! compiled in every build; this directory is compiled only with the
//! `diarization` feature (#372).

pub mod clustering;
pub mod embedding;
pub mod fbank;
pub mod segmentation;

/// Keep both CPU-bound ONNX stages within the Apple-Silicon performance-core
/// budget while Whisper runs concurrently on Metal.
const ORT_INTRA_THREADS: usize = 4;
